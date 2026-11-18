//! A default getter derived from the second formal of a curried member.
use super::*;

#[derive(Clone, Copy)]
pub(crate) struct ContextualDefault {
    pub(crate) defaulted: SymbolId,
    pub(crate) getter: SymbolId,
    pub(crate) copied: SymbolId,
    pub(crate) default_tree: TreeId<Untyped>,
    pub(crate) previous_tree: TreeId<Untyped>,
    pub(crate) body: TreeId<Typed>,
}

impl Validator<'_, '_> {
    pub(super) fn contextual_default(
        &mut self,
        method: &MethodPicklePlan,
    ) -> Result<Option<ContextualDefault>, PickleError> {
        let p = self.plan;
        let TreeKind::DefDef(def) = &self.source_tree(method.source_tree)?.kind else {
            return Err(PickleError::SourceShape {
                tree: method.source_tree,
                expected: SourceRole::Method,
            });
        };
        let [first, second] = def.value_param_clauses.as_slice() else {
            return Ok(None);
        };
        let ([previous], [default]) = (first.as_slice(), second.as_slice()) else {
            return Ok(None);
        };
        let TreeKind::ValDef(value) = &self.source_tree(*default)?.kind else {
            return Ok(None);
        };
        let Some(rhs) = value.rhs else {
            return Ok(None);
        };
        let index = p.source_index;
        let source = p.source.id;
        let Some(getter) = index.derived_symbol_at(p.class, source, *default) else {
            return Ok(None);
        };
        let invalid = || PickleError::SourceDefinition { symbol: getter };
        self.charge(4)?;
        let defaulted = index.symbol_at(source, *default).ok_or_else(invalid)?;
        let original = index.symbol_at(source, *previous).ok_or_else(invalid)?;
        let copied = index
            .derived_symbol_at(getter, source, *previous)
            .ok_or_else(invalid)?;
        let copy = self.symbol(copied)?;
        let getter_record = self.symbol(getter)?;
        let int = p.definitions.int;
        let unsupported = || PickleError::Unsupported {
            feature: UnsupportedFeature::ParameterClauses,
            symbol: Some(defaulted),
        };
        if !def.type_params.is_empty()
            || self.symbol(defaulted)?.flags != SymbolFlags::GIVEN
            || !self.symbol(original)?.flags.is_empty()
            || self.complete(original)? != int
            || self.complete(defaulted)? != int
        {
            return Err(unsupported());
        }
        if p.store.default_getter(method.symbol, 1) != Some(getter)
            || p.store.default_getter_origin(getter) != Some((method.symbol, 1))
            || index.definition_of(getter)
                != Some(SourceDefinition::Derived {
                    source,
                    tree: *default,
                })
            || index.definition_of(copied)
                != Some(SourceDefinition::Derived {
                    source,
                    tree: *previous,
                })
            || copy.name != self.symbol(original)?.name
            || !copy.flags.is_empty()
            || getter_record.flags != SymbolFlags::SYNTHETIC
            || !getter_record.name.is_term()
            || self.name(getter_record.name)?
                != format!("{}$default$2", self.name(self.symbol(method.symbol)?.name)?)
        {
            return Err(invalid());
        }
        for (symbol, owner, kind) in [
            (getter, p.class, SymbolKind::Method),
            (copied, getter, SymbolKind::Parameter),
        ] {
            self.kind(symbol, kind)?;
            self.owner(symbol, Some(owner))?;
            let record = self.symbol(symbol)?;
            if record.origin != SymbolOrigin::Source(source)
                || record.visibility != Visibility::Public
                || !record.annotations.is_empty()
                || self.scope(owner)?.lookup_all(&record.name) != [symbol]
            {
                return Err(invalid());
            }
        }
        if !self.scope(getter)?.entered_symbols().eq([copied]) {
            return Err(PickleError::ScopeMembers { owner: getter });
        }
        let ty = self.complete(getter)?;
        let Type::Method(signature) = self.ty(ty)? else {
            return Err(invalid());
        };
        if !matches!(signature.params.as_slice(),[param] if param.ty==int && param.name.as_name()==&copy.name && !param.varargs && !param.erased)
            || signature.kind != MethodKind::Plain
            || signature.result != int
            || self.complete(copied)? != int
        {
            return Err(invalid());
        }
        let body = p
            .typed_bindings
            .typed_at(source, rhs)
            .ok_or(PickleError::MissingBody { method: getter })?;
        let node = p
            .typed_ast
            .try_get(body)
            .ok_or(PickleError::InvalidTypedTree { tree: body })?;
        if !matches!(&self.source_tree(rhs)?.kind,TreeKind::Ident(id) if id.name==copy.name) {
            return Err(unsupported());
        }
        if !matches!(&node.kind, TreeKind::Ident(_))
            || self.ty(node.ty)?.reference_symbol() != Some(copied)
        {
            return Err(PickleError::BodyType {
                tree: body,
                ty: node.ty,
            });
        }
        self.validate_method_expression(
            Some(rhs),
            Some(body),
            &MethodPicklePlan {
                symbol: getter,
                source_tree: method.source_tree,
                parameter_clauses: vec![vec![copied]],
                body: Some(body),
            },
            int,
        )?;
        Ok(Some(ContextualDefault {
            defaulted,
            getter,
            copied,
            default_tree: *default,
            previous_tree: *previous,
            body,
        }))
    }
}
