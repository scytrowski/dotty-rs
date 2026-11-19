//! Identity and bounds checks for a polymorphic contextual identity method.

use super::*;

impl Validator<'_, '_> {
    pub(super) fn polymorphic_evidence_method(
        &mut self,
        definition: &DefDef<Untyped>,
        method: &MethodPicklePlan,
        signature_only: bool,
    ) -> Result<(), PickleError> {
        self.charge(3)?; // One type parameter, one term clause, and one evidence parameter.
        let unsupported =
            || self.unsupported(UnsupportedFeature::TypeParameters, Some(method.symbol));
        let [type_tree] = definition.type_params.as_slice() else {
            return Err(unsupported());
        };
        let [clause] = definition.value_param_clauses.as_slice() else {
            return Err(unsupported());
        };
        let [term_tree] = clause.as_slice() else {
            return Err(unsupported());
        };
        let type_parameter = self
            .plan
            .source_index
            .symbol_at(self.plan.source.id, *type_tree)
            .ok_or_else(unsupported)?;
        let evidence = self
            .plan
            .source_index
            .symbol_at(self.plan.source.id, *term_tree)
            .ok_or_else(unsupported)?;
        let TreeKind::TypeDef(tparam) = &self.source_tree(*type_tree)?.kind else {
            return Err(unsupported());
        };
        let TreeKind::TypeBoundsTree(bounds) = &self.source_tree(tparam.rhs)?.kind else {
            return Err(unsupported());
        };
        let TreeKind::ValDef(param) = &self.source_tree(*term_tree)?.kind else {
            return Err(unsupported());
        };
        let signature = self.complete(method.symbol)?;
        let Type::Poly(poly) = self.ty(signature)? else {
            return Err(unsupported());
        };
        let [type_formal] = poly.params.as_slice() else {
            return Err(unsupported());
        };
        let Type::Method(contextual) = self.ty(poly.result)? else {
            return Err(unsupported());
        };
        let [formal] = contextual.params.as_slice() else {
            return Err(unsupported());
        };
        let result_type = self.complete(evidence)?;
        let default_bounds = Type::Bounds {
            low: self.plan.definitions.nothing_type,
            high: self.plan.definitions.any_type,
        };
        let valid_type_parameter = definition.source_param_clause_order.is_none()
            && tparam.variance.is_none()
            && bounds.low.is_none()
            && bounds.high.is_none()
            && bounds.alias.is_none()
            && type_formal.name == tparam.name
            && type_formal.declared_variance.is_none()
            && self.ty(type_formal.bounds)? == &default_bounds
            && self.ty(self.complete(type_parameter)?)? == &default_bounds;
        let valid_evidence = matches!(method.parameter_clauses.as_slice(), [clause] if clause.as_slice() == [evidence])
            && param.rhs.is_none()
            && contextual.kind == MethodKind::Contextual
            && self.symbol(evidence)?.flags == SymbolFlags::GIVEN
            && formal.name == param.name
            && !formal.erased
            && !formal.varargs;
        let bound_reference = Type::ParamRef {
            binder: signature,
            index: 0,
        };
        let canonical_reference = Type::type_ref(self.plan.definitions.no_prefix, type_parameter);
        let mut matching_source_types = true;
        for tree in [definition.tpt, param.tpt] {
            matching_source_types &= matches!(
                &self.source_tree(tree)?.kind,
                TreeKind::Ident(id) if id.name == *tparam.name.as_name()
            );
        }
        let matching_types = self.ty(formal.ty)? == &bound_reference
            && self.ty(contextual.result)? == &bound_reference
            && self.ty(result_type)? == &canonical_reference
            && matching_source_types;
        if !(valid_type_parameter && valid_evidence && matching_types) {
            return Err(unsupported());
        }
        self.source_symbol(
            type_parameter,
            *type_tree,
            SymbolKind::TypeParameter,
            *tparam.name.as_name(),
        )?;
        self.source_symbol(
            evidence,
            *term_tree,
            SymbolKind::Parameter,
            *param.name.as_name(),
        )?;
        self.parameter_metadata(&tparam.metadata, type_parameter)?;
        self.parameter_metadata(&param.metadata, evidence)?;
        for symbol in [type_parameter, evidence] {
            self.owner(symbol, Some(method.symbol))?;
            if self
                .scope(method.symbol)?
                .lookup_all(&self.symbol(symbol)?.name)
                != [symbol]
            {
                return Err(PickleError::ScopeMembers {
                    owner: method.symbol,
                });
            }
        }
        if signature_only {
            if definition.rhs.is_none() || method.body.is_none() {
                return Err(PickleError::MissingBody {
                    method: method.symbol,
                });
            }
            return Ok(());
        }
        self.validate_method_expression(definition.rhs, method.body, method, result_type)
    }
}
