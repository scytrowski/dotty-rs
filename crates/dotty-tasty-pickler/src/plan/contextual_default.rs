//! Checked source and typed identities for supported contextual default getters.
use super::*;

#[derive(Clone, Copy)]
pub(crate) struct GetterParameter {
    pub(crate) copied: SymbolId,
    pub(crate) source_tree: TreeId<Untyped>,
}

#[derive(Clone, Copy)]
pub(crate) struct ContextualDefault {
    pub(crate) defaulted: SymbolId,
    pub(crate) getter: SymbolId,
    pub(crate) previous: Option<GetterParameter>,
    pub(crate) default_tree: TreeId<Untyped>,
    pub(crate) body: TreeId<Typed>,
}

impl ContextualDefault {
    pub(crate) fn ordinal(&self) -> u32 {
        u32::from(self.previous.is_some())
    }
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
        let (previous, default_clause) = match def.value_param_clauses.as_slice() {
            [only] => (None, only),
            [first, second] if first.len() == 1 => (Some(first[0]), second),
            _ => return Ok(None),
        };
        let [default] = default_clause.as_slice() else {
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
        self.charge(4)?;
        let invalid = || PickleError::SourceDefinition { symbol: getter };
        let defaulted = index.symbol_at(source, *default).ok_or_else(invalid)?;
        let unsupported = || PickleError::Unsupported {
            feature: UnsupportedFeature::ParameterClauses,
            symbol: Some(defaulted),
        };
        let int = p.definitions.int;
        if !def.type_params.is_empty()
            || !matches!(
                self.symbol(defaulted)?.flags,
                SymbolFlags::GIVEN | SymbolFlags::IMPLICIT
            )
            || self.complete(defaulted)? != int
        {
            return Err(unsupported());
        }
        let ordinal = usize::from(previous.is_some());
        self.getter_declaration(
            getter,
            p.class,
            *default,
            SymbolKind::Method,
            SymbolFlags::SYNTHETIC,
        )?;
        if p.store.default_getter(method.symbol, ordinal) != Some(getter)
            || p.store.default_getter_origin(getter) != Some((method.symbol, ordinal))
            || self.name(self.symbol(getter)?.name)?
                != format!(
                    "{}$default${}",
                    self.name(self.symbol(method.symbol)?.name)?,
                    ordinal + 1
                )
        {
            return Err(invalid());
        }
        let previous = if let Some(previous_tree) = previous {
            Some(self.getter_parameter(getter, previous_tree, defaulted)?)
        } else {
            // A parameterless getter carries its result directly. An empty method
            // clause would denote a different callable in Scala and on the wire.
            if self.complete(getter)? != int {
                return Err(invalid());
            }
            None
        };
        let parameter = previous.map(|p| p.copied);
        if !self.scope(getter)?.entered_symbols().eq(parameter) {
            return Err(PickleError::ScopeMembers { owner: getter });
        }
        let body = p
            .typed_bindings
            .typed_at(source, rhs)
            .ok_or(PickleError::MissingBody { method: getter })?;
        let node = p
            .typed_ast
            .try_get(body)
            .ok_or(PickleError::InvalidTypedTree { tree: body })?;
        if let Some(copied) = parameter {
            if !matches!(&self.source_tree(rhs)?.kind,TreeKind::Ident(id) if id.name==self.symbol(copied)?.name)
            {
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
        } else {
            if !matches!(
                self.source_tree(rhs)?.kind,
                TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_))
            ) {
                return Err(unsupported());
            }
            if !matches!(&node.kind,TreeKind::Literal(literal) if matches!(literal.value,Constant::Int(_)))
            {
                return Err(PickleError::BodyType {
                    tree: body,
                    ty: node.ty,
                });
            }
        }
        self.validate_method_expression(
            Some(rhs),
            Some(body),
            &MethodPicklePlan {
                symbol: getter,
                source_tree: method.source_tree,
                parameter_clauses: parameter.into_iter().map(|p| vec![p]).collect(),
                body: Some(body),
            },
            int,
        )?;
        Ok(Some(ContextualDefault {
            defaulted,
            getter,
            previous,
            default_tree: *default,
            body,
        }))
    }

    fn getter_parameter(
        &self,
        getter: SymbolId,
        source_tree: TreeId<Untyped>,
        defaulted: SymbolId,
    ) -> Result<GetterParameter, PickleError> {
        let p = self.plan;
        let source = p.source.id;
        let invalid = || PickleError::SourceDefinition { symbol: getter };
        let original = p
            .source_index
            .symbol_at(source, source_tree)
            .ok_or_else(invalid)?;
        let copied = p
            .source_index
            .derived_symbol_at(getter, source, source_tree)
            .ok_or_else(invalid)?;
        self.getter_declaration(
            copied,
            getter,
            source_tree,
            SymbolKind::Parameter,
            SymbolFlags::EMPTY,
        )?;
        let int = p.definitions.int;
        if !self.symbol(original)?.flags.is_empty() || self.complete(original)? != int {
            return Err(self.unsupported(UnsupportedFeature::ParameterClauses, Some(defaulted)));
        }
        let Type::Method(signature) = self.ty(self.complete(getter)?)? else {
            return Err(invalid());
        };
        let [param] = signature.params.as_slice() else {
            return Err(invalid());
        };
        let name = self.symbol(copied)?.name;
        if name != self.symbol(original)?.name
            || self.complete(copied)? != int
            || signature.kind != MethodKind::Plain
            || signature.result != int
            || param.name.as_name() != &name
            || param.ty != int
            || param.erased
            || param.varargs
        {
            return Err(invalid());
        }
        Ok(GetterParameter {
            copied,
            source_tree,
        })
    }

    fn getter_declaration(
        &self,
        symbol: SymbolId,
        owner: SymbolId,
        tree: TreeId<Untyped>,
        kind: SymbolKind,
        flags: SymbolFlags,
    ) -> Result<(), PickleError> {
        let source = self.plan.source.id;
        self.kind(symbol, kind)?;
        self.owner(symbol, Some(owner))?;
        let record = self.symbol(symbol)?;
        if self.plan.source_index.definition_of(symbol)
            != Some(SourceDefinition::Derived { source, tree })
            || record.origin != SymbolOrigin::Source(source)
            || !record.name.is_term()
            || record.flags != flags
            || record.visibility != Visibility::Public
            || !record.annotations.is_empty()
            || self.scope(owner)?.lookup_all(&record.name) != [symbol]
        {
            return Err(PickleError::SourceDefinition { symbol });
        }
        Ok(())
    }
}
