//! Placement of source context-bound witnesses in method parameter clauses.
use super::{AstArena, DefDef, Modifier, TreeKind, Untyped, UntypedNode};
use crate::{Name, NameInterner, TermName, TreeId};
use std::collections::HashSet;

impl DefDef<Untyped> {
    /// Context-bound witness trees on this definition's type parameters, in source order.
    pub fn context_bound_trees<'a>(
        &'a self,
        arena: &'a AstArena<Untyped>,
    ) -> impl Iterator<Item = TreeId<Untyped>> + 'a {
        self.type_params
            .iter()
            .filter_map(|tree| {
                let TreeKind::TypeDef(param) = &arena.try_get(*tree)?.kind else {
                    return None;
                };
                let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(bounds)) =
                    &arena.try_get(param.rhs)?.kind
                else {
                    return None;
                };
                Some(bounds.context_bounds.as_slice())
            })
            .flatten()
            .copied()
    }

    /// Scala 3.9 Desugar.fitEvidenceParams. Keep the source-bound tree as the
    /// identity of each witness across naming, signature completion and DefDef typing.
    pub fn context_bound_clauses(
        &self,
        arena: &AstArena<Untyped>,
        interner: &NameInterner,
    ) -> std::borrow::Cow<'_, [Vec<TreeId<Untyped>>]> {
        let mut witnesses = Vec::new();
        let mut names = HashSet::new();
        let mut anonymous = 0;
        for tree in self.context_bound_trees(arena) {
            witnesses.push(tree);
            if let Some(TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(bound))) =
                arena.try_get(tree).map(|n| &n.kind)
            {
                let name = bound.name.or_else(|| {
                    anonymous += 1;
                    interner
                        .get(&format!("evidence${anonymous}"))
                        .map(TermName::new)
                });
                if let Some(name) = name {
                    names.insert(*name.as_name());
                }
            }
        }
        if witnesses.is_empty() {
            return std::borrow::Cow::Borrowed(&self.value_param_clauses);
        }
        let mut clauses = self.value_param_clauses.clone();
        if let Some(index) = clauses.iter().position(|clause| {
            clause.iter().any(|parameter| {
                matches!(arena.try_get(*parameter).map(|n| &n.kind), Some(TreeKind::ValDef(param))
                    if type_references_witness(arena, param.tpt, &names))
            })
        }) {
            clauses.insert(index, witnesses);
        } else if let Some(last) = clauses.last_mut()
            && last.first().is_some_and(|tree| {
                matches!(arena.try_get(*tree).map(|n| &n.kind), Some(TreeKind::ValDef(v))
                    if v.metadata.modifiers.contains(&Modifier::Given) || v.metadata.modifiers.contains(&Modifier::Implicit))
            }) {
            witnesses.append(last);
            *last=witnesses;
        } else {
            clauses.push(witnesses);
        }
        std::borrow::Cow::Owned(clauses)
    }
}

fn type_references_witness(
    arena: &AstArena<Untyped>,
    root: TreeId<Untyped>,
    names: &HashSet<Name>,
) -> bool {
    let mut pending = vec![root];
    let mut seen = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !seen.insert(tree) {
            continue;
        }
        let Some(node) = arena.try_get(tree) else {
            continue;
        };
        match &node.kind {
            TreeKind::Ident(id) if names.contains(&id.name) => return true,
            TreeKind::Select(s) => {
                // Dotted type paths carry type names even for a stable term prefix.
                if !s.is_type_projection
                    && let Some(TreeKind::Ident(id)) = arena.try_get(s.qualifier).map(|n| &n.kind)
                    && names.contains(TermName::new(id.name.text()).as_name())
                {
                    return true;
                }
                pending.push(s.qualifier);
            }
            TreeKind::AppliedTypeTree(t) => {
                pending.push(t.tpt);
                pending.extend(&t.args);
            }
            TreeKind::SingletonTypeTree(t) => pending.push(t.reference),
            TreeKind::ByNameTypeTree(t) => pending.push(t.result),
            TreeKind::TypeBoundsTree(t) => {
                pending.extend([t.low, t.high, t.alias].into_iter().flatten())
            }
            TreeKind::RefinedTypeTree(t) => {
                pending.push(t.tpt);
                pending.extend(&t.refinements);
            }
            TreeKind::ValDef(v) => pending.push(v.tpt),
            TreeKind::TypeDef(t) => pending.push(t.rhs),
            TreeKind::LambdaTypeTree(t) => {
                pending.push(t.body);
                pending.extend(&t.type_params);
            }
            TreeKind::MatchTypeTree(t) => {
                pending.extend(t.bound);
                pending.push(t.selector);
                pending.extend(&t.cases);
            }
            TreeKind::CaseDef(t) => pending.extend([t.pattern, t.body]),
            TreeKind::Annotated(t) => pending.push(t.expr),
            TreeKind::PhaseSpecific(UntypedNode::Function(t)) => {
                pending.extend(&t.params);
                pending.push(t.body);
            }
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(t)) => {
                pending.extend(&t.params);
                pending.push(t.result);
            }
            TreeKind::PhaseSpecific(UntypedNode::Tuple(t)) => pending.extend(&t.elements),
            TreeKind::PhaseSpecific(UntypedNode::Parens(t)) => pending.push(t.inner),
            TreeKind::PhaseSpecific(UntypedNode::PostfixOp(t)) => pending.push(t.operand),
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(t)) => pending.extend([t.left, t.right]),
            _ => {}
        }
    }
    false
}
