//! Exact source/typed validation for the bounded signed 64-bit value slice.
use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_long_value(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        depth: usize,
        visited: &mut HashSet<TreeId<Typed>>,
    ) -> Result<(), PickleError> {
        if depth > self.limits.expression_depth_limit() {
            return Err(PickleError::LimitExceeded {
                kind: LimitKind::ExpressionDepth,
                limit: self.limits.expression_depth_limit(),
            });
        }
        let p = self.plan;
        let source = self.source_tree(source_id)?;
        let typed = p
            .typed_ast
            .try_get(typed_id)
            .ok_or(PickleError::InvalidTypedTree { tree: typed_id })?;
        self.source_typed_binding(source_id, typed_id)?;
        if let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &source.kind {
            self.charge(1)?;
            return self.validate_long_value(parens.inner, typed_id, method, depth + 1, visited);
        }
        if matches!(
            typed.kind,
            TreeKind::Block(_) | TreeKind::If(_) | TreeKind::Match(_)
        ) {
            // Value positions can contain ordinary control flow, but not a
            // nested return that would bypass the initializer/assignment/value.
            if self.contains_local_return(source_id, typed_id) {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
            return self
                .validate_return_flow(
                    source_id,
                    typed_id,
                    method,
                    p.definitions.long,
                    depth,
                    visited,
                )
                .map(|_| ());
        }
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        match (&source.kind, &typed.kind) {
            (TreeKind::Literal(source), TreeKind::Literal(value)) => {
                let (Constant::Long(expected), Constant::Long(actual)) =
                    (&source.value, &value.value)
                else {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                };
                if !matches!(self.ty(typed.ty)?, Type::Constant(Constant::Long(value)) if value == actual)
                {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                if expected != actual {
                    return Err(PickleError::SourceShape {
                        tree: source_id,
                        expected: SourceRole::Body,
                    });
                }
            }
            (TreeKind::Ident(source), TreeKind::Ident(value))
                if source.name == value.name && source.backquoted == value.backquoted =>
            {
                let Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                } = self.ty(typed.ty)?
                else {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                };
                let symbol = self.symbol(*target)?;
                let formal = symbol.kind == SymbolKind::Parameter
                    && method
                        .parameter_clauses
                        .iter()
                        .flatten()
                        .any(|s| s == target);
                let local = symbol.kind == SymbolKind::Local
                    && self.registered_local_symbols.contains(target)
                    && self.visible_locals.contains(target);
                if !formal && !local
                    || symbol.name != value.name
                    || symbol.owner != Some(method.symbol)
                    || *prefix != p.definitions.no_prefix
                    || !matches!(self.ty(*prefix)?, Type::NoPrefix)
                    || self.complete(*target)? != p.definitions.long
                {
                    return Err(PickleError::Reference {
                        symbol: *target,
                        reason: crate::ReferenceError::NotRegistered,
                    });
                }
            }
            _ => {
                return Err(
                    self.unsupported(UnsupportedFeature::BodyExpression, Some(method.symbol))
                );
            }
        }
        Ok(())
    }
}
