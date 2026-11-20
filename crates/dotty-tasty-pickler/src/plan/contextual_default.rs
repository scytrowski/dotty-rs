//! Checked source and typed identities for supported contextual default getters.
use super::*;

pub(crate) struct GetterParameter {
    pub(crate) copied: SymbolId,
    pub(crate) source_tree: TreeId<Untyped>,
}

pub(crate) struct ContextualDefault {
    pub(crate) defaulted: SymbolId,
    pub(crate) getter: SymbolId,
    pub(crate) previous: Vec<GetterParameter>,
    pub(crate) default_tree: TreeId<Untyped>,
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
        let (previous, default_clause) = match def.value_param_clauses.as_slice() {
            [only] => (&[][..], only),
            [first, second] if matches!(first.len(), 1 | 2) => (first.as_slice(), second),
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
        self.charge(4 + previous.len().saturating_sub(1))?;
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
        let ordinal = previous.len();
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
        let previous = self.getter_parameters(getter, previous, defaulted)?;
        let parameters: Vec<_> = previous.iter().map(|p| p.copied).collect();
        // Each copy was checked in its own name bucket. Scope iteration is
        // unordered; the getter signature and source clause establish order.
        let scope = self.scope(getter)?;
        if scope.entered_symbols().take(parameters.len() + 1).count() != parameters.len() {
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
        if !parameters.is_empty() {
            let TreeKind::Ident(id) = &self.source_tree(rhs)?.kind else {
                return Err(unsupported());
            };
            let &[copied] = scope.lookup_all(&id.name) else {
                return Err(unsupported());
            };
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
                parameter_clauses: if parameters.is_empty() {
                    Vec::new()
                } else {
                    vec![parameters]
                },
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

    fn getter_parameters(
        &self,
        getter: SymbolId,
        preceding_clause: &[TreeId<Untyped>],
        defaulted: SymbolId,
    ) -> Result<Vec<GetterParameter>, PickleError> {
        let p = self.plan;
        let int = p.definitions.int;
        let invalid = || PickleError::SourceDefinition { symbol: getter };
        let info = self.complete(getter)?;
        if preceding_clause.is_empty() {
            // A parameterless getter carries its result type directly.
            return if info == int {
                Ok(Vec::new())
            } else {
                Err(invalid())
            };
        }
        let Type::Method(signature) = self.ty(info)? else {
            return Err(invalid());
        };
        if signature.kind != MethodKind::Plain
            || signature.result != int
            || signature.params.len() != preceding_clause.len()
        {
            return Err(invalid());
        }
        let mut copies = Vec::with_capacity(preceding_clause.len());
        for (&source_tree, formal) in preceding_clause.iter().zip(&signature.params) {
            let original = p
                .source_index
                .symbol_at(p.source.id, source_tree)
                .ok_or_else(invalid)?;
            let copied = p
                .source_index
                .derived_symbol_at(getter, p.source.id, source_tree)
                .ok_or_else(invalid)?;
            self.getter_declaration(
                copied,
                getter,
                source_tree,
                SymbolKind::Parameter,
                SymbolFlags::EMPTY,
            )?;
            if !self.symbol(original)?.flags.is_empty() || self.complete(original)? != int {
                return Err(self.unsupported(UnsupportedFeature::ParameterClauses, Some(defaulted)));
            }
            let name = self.symbol(copied)?.name;
            if name != self.symbol(original)?.name
                || self.complete(copied)? != int
                || formal.name.as_name() != &name
                || formal.ty != int
                || formal.erased
                || formal.varargs
            {
                return Err(invalid());
            }
            copies.push(GetterParameter {
                copied,
                source_tree,
            });
        }
        Ok(copies)
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
