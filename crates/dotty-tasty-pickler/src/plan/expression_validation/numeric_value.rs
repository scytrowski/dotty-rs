//! Exact source/typed validation for bounded Long/Float/Double method values.
use super::*;

impl Validator<'_, '_> {
    pub(super) fn validate_numeric_value(
        &mut self,
        source_id: TreeId<Untyped>,
        typed_id: TreeId<Typed>,
        method: &MethodPicklePlan,
        result_type: TypeId,
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
            return self.validate_numeric_value(
                parens.inner,
                typed_id,
                method,
                result_type,
                depth + 1,
                visited,
            );
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
                .validate_return_flow(source_id, typed_id, method, result_type, depth, visited)
                .map(|_| ());
        }
        self.paired_source_typed_span(source.position, typed.position, typed_id)?;
        self.charge(1)?;
        if !visited.insert(typed_id) {
            return Err(PickleError::RepeatedExpression { tree: typed_id });
        }
        match (&source.kind, &typed.kind) {
            (TreeKind::Literal(source), TreeKind::Literal(value)) => {
                let valid = match (&value.value, self.ty(typed.ty)?) {
                    (Constant::Long(actual), Type::Constant(Constant::Long(ty))) => {
                        result_type == p.definitions.long && actual == ty
                    }
                    (Constant::FloatBits(actual), Type::Constant(Constant::FloatBits(ty))) => {
                        result_type == p.definitions.float && actual == ty
                    }
                    (Constant::DoubleBits(actual), Type::Constant(Constant::DoubleBits(ty))) => {
                        result_type == p.definitions.double && actual == ty
                    }
                    _ => false,
                };
                if !valid {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                // Constant equality compares raw bits, including signed zero and NaNs.
                if source.value != value.value {
                    return Err(PickleError::SourceShape {
                        tree: source_id,
                        expected: SourceRole::Body,
                    });
                }
            }
            (TreeKind::PhaseSpecific(UntypedNode::Number(number)), TreeKind::Literal(value)) => {
                // Unsuffixed decimal/exponent syntax remains raw in the source AST.
                // Authenticate its spelling with the same finite f64 conversion as
                // the typer, after applying the existing bounded-name check.
                if result_type != p.definitions.double
                    || !matches!(number.kind, NumberKind::Decimal | NumberKind::Floating)
                {
                    return Err(PickleError::SourceShape {
                        tree: source_id,
                        expected: SourceRole::Body,
                    });
                }
                let Constant::DoubleBits(actual) = value.value else {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                };
                if !matches!(self.ty(typed.ty)?, Type::Constant(Constant::DoubleBits(bits)) if *bits == actual)
                {
                    return Err(PickleError::BodyType {
                        tree: typed_id,
                        ty: typed.ty,
                    });
                }
                let bits = self
                    .name_text(number.text)?
                    .replace('_', "")
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(f64::to_bits);
                if bits != Some(actual) {
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
                    || self.complete(*target)? != result_type
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
