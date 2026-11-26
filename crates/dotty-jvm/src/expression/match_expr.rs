use super::*;

struct MatchCase {
    body: TreeId<Typed>,
    literals: Vec<i32>,
    guard: Option<TreeId<Typed>>,
    binder: Option<JvmLocalSlot>,
}

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
    // Bound the allocation before cloning an externally supplied case vector.
    if matched.cases.len() > JVM_EXPRESSION_NODE_BUDGET.saturating_sub(context.node_visits) {
        return Err(JvmCodegenError::ExpressionNodeBudgetExceeded {
            tree,
            limit: JVM_EXPRESSION_NODE_BUDGET,
        });
    }
    let matched = matched.clone();
    let expected = if is_nothing_type(context.store, node.ty, context.definitions) {
        Flow::Diverges
    } else if is_unit_type(context.store, node.ty, context.definitions) {
        Flow::Unit
    } else {
        match primitive_type(context.store, node.ty, context.definitions) {
            Some(
                value @ (JvmType::Int
                | JvmType::Boolean
                | JvmType::Long
                | JvmType::Float
                | JvmType::Double),
            ) => Flow::Value(value),
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
        if case_node.ty != context.tree(case.body)?.ty {
            return Err(unsupported(case_id, "inconsistent match case type"));
        }
        context.visit(case.pattern, depth + 2)?;
        let mut pattern_id = case.pattern;
        let mut binder = None;
        if let TreeKind::Bind(bind) = context.tree(pattern_id)?.kind {
            if bind.given {
                return Err(unsupported(pattern_id, "given match binder"));
            }
            let Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } = context
                .store
                .types
                .try_get(context.tree(pattern_id)?.ty)
                .ok_or_else(|| unsupported(pattern_id, "invalid match binder"))?
            else {
                return Err(unsupported(pattern_id, "invalid match binder"));
            };
            let symbol = *symbol;
            let layout = context
                .local_layout
                .ok_or(JvmCodegenError::MissingLocalBinding {
                    tree: pattern_id,
                    symbol,
                })?;
            let local =
                *layout
                    .local(symbol)
                    .map_err(|_| JvmCodegenError::MissingLocalBinding {
                        tree: pattern_id,
                        symbol,
                    })?;
            if local.declaration() != pattern_id
                || local.ty() != JvmType::Int
                || !local
                    .pattern_case()
                    .is_some_and(|(_, case)| case == case_id)
            {
                return Err(unsupported(pattern_id, "inconsistent match binder"));
            }
            binder = Some(local);
            pattern_id = bind.body;
            context.visit(pattern_id, depth + 3)?;
        }
        let pattern = context.tree(pattern_id)?;
        let pattern_type = pattern.ty;
        let literals = match &pattern.kind {
            TreeKind::Literal(literal) if matches!(literal.value, Constant::Int(_)) => {
                let Constant::Int(value) = literal.value else {
                    unreachable!()
                };
                if !matches!(context.store.types.try_get(pattern.ty), Some(Type::Constant(Constant::Int(typed))) if *typed == value)
                {
                    return Err(unsupported(case.pattern, "inconsistent Int match literal"));
                }
                vec![value]
            }
            TreeKind::Ident(ident)
                if !ident.backquoted
                    && ident.name.is_term()
                    && context.store.names.try_resolve(ident.name.text()) == Some("_") =>
            {
                Vec::new()
            }
            TreeKind::Alternative(_) if binder.is_none() => {
                let mut literals = Vec::new();
                collect_alternative_literals(context, pattern_id, depth + 2, &mut literals)?;
                literals
            }
            _ => return Err(unsupported(case.pattern, "unsupported match pattern")),
        };
        if (literals.is_empty() && case.guard.is_none()) != (index + 1 == matched.cases.len()) {
            return Err(unsupported(
                case.pattern,
                "match requires exactly one final wildcard",
            ));
        }
        if primitive_type(context.store, pattern_type, context.definitions) != Some(JvmType::Int) {
            return Err(unsupported(case.pattern, "non-Int match pattern type"));
        }
        cases.push(MatchCase {
            body: case.body,
            literals,
            guard: case.guard,
            binder,
        });
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
    cases: &[MatchCase],
    expected: Flow,
    slot: usize,
    code: &mut CodeBuilder,
    depth: usize,
) -> Result<Flow, JvmCodegenError> {
    let build_error = |error| JvmCodegenError::CodeBuilder { tree, error };
    code.istore(slot).map_err(build_error)?;
    let mut join: Option<dotty_classfile::bytecode::CodeLabel> = None;
    let before = context.initialized_locals.clone();
    let mut joined: Option<HashSet<SymbolId>> = None;
    let mut incoming = before.clone();
    for case in cases {
        let MatchCase {
            body,
            guard,
            binder,
            ..
        } = *case;
        let literals = &case.literals;
        context.initialized_locals = incoming.clone();
        let next = if !literals.is_empty() || guard.is_some() {
            Some(code.new_label().map_err(build_error)?)
        } else {
            None
        };
        if !literals.is_empty() {
            let next = next.as_ref().expect("literal has a continuation");
            // All alternatives enter one shared guard/body. A false guard must
            // skip the entire case, even if a later alternative also matches.
            let hit = if literals.len() > 1 {
                Some(code.new_label().map_err(build_error)?)
            } else {
                None
            };
            for (ordinal, &value) in literals.iter().enumerate() {
                code.iload(slot)
                    .and_then(|c| c.iconst(value))
                    .map_err(build_error)?;
                if ordinal + 1 == literals.len() {
                    code.if_icmpne(next.clone()).map_err(build_error)?;
                } else {
                    code.if_icmpeq(hit.as_ref().expect("multiple alternatives").clone())
                        .map_err(build_error)?;
                }
            }
            if let Some(hit) = hit {
                code.mark(hit).map_err(build_error)?;
            }
        }
        if let Some(local) = binder {
            code.iload(slot)
                .and_then(|c| c.istore(usize::from(local.slot())))
                .map_err(build_error)?;
            context.initialized_locals.insert(local.symbol());
        }
        if let Some(guard) = guard {
            if emit_expression(context, guard, code, depth + 2)? != Flow::Value(JvmType::Boolean) {
                return Err(unsupported(guard, "non-Boolean match guard"));
            }
            code.ifeq(next.as_ref().expect("guard has a continuation").clone())
                .map_err(build_error)?;
            let mut after_guard = context.initialized_locals.clone();
            if let Some(local) = binder {
                after_guard.remove(&local.symbol());
            }
            incoming = if !literals.is_empty() {
                incoming.intersection(&after_guard).copied().collect()
            } else {
                after_guard
            };
        }
        let flow = emit_expression(context, body, code, depth + 2)?;
        validate_branch(context, body, flow)?;
        if let Some(local) = binder {
            context.initialized_locals.remove(&local.symbol());
        }
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
                let target = if let Some(join) = &join {
                    join.clone()
                } else {
                    let label = code.new_label().map_err(build_error)?;
                    join = Some(label.clone());
                    label
                };
                code.goto(target).map_err(build_error)?;
            }
        }
        if let Some(next) = next {
            code.mark(next).map_err(build_error)?;
        }
    }
    if let Some(join) = join {
        code.mark(join).map_err(build_error)?;
    }
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

// The root was charged by emit_match; every descendant is charged exactly once.
// Flatten only literal tests, preserving one guard/body and the expanded node budget.
fn collect_alternative_literals(
    context: &mut JvmLoweringContext<'_>,
    root: TreeId<Typed>,
    depth: usize,
    literals: &mut Vec<i32>,
) -> Result<(), JvmCodegenError> {
    let node = context.tree(root)?;
    match &node.kind {
        TreeKind::Literal(literal) => {
            let Constant::Int(value) = literal.value else {
                return Err(unsupported(root, "unsupported Int match alternative"));
            };
            if !matches!(context.store.types.try_get(node.ty), Some(Type::Constant(Constant::Int(typed))) if *typed == value)
            {
                return Err(unsupported(root, "inconsistent Int match literal"));
            }
            literals.push(value);
        }
        TreeKind::Alternative(alternative) => {
            if node.ty != context.definitions.int || alternative.alternatives.is_empty() {
                return Err(unsupported(root, "invalid Int match alternative"));
            }
            let count = alternative.alternatives.len();
            if count > JVM_EXPRESSION_NODE_BUDGET.saturating_sub(context.node_visits) {
                return Err(JvmCodegenError::ExpressionNodeBudgetExceeded {
                    tree: root,
                    limit: JVM_EXPRESSION_NODE_BUDGET,
                });
            }
            for ordinal in 0..count {
                let TreeKind::Alternative(alternative) = &context.tree(root)?.kind else {
                    unreachable!("checked alternative");
                };
                let child = alternative.alternatives[ordinal];
                context.visit(child, depth + 1)?;
                collect_alternative_literals(context, child, depth + 1, literals)?;
            }
        }
        _ => return Err(unsupported(root, "unsupported Int match alternative")),
    }
    Ok(())
}
