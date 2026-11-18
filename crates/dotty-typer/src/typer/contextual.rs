//! Context argument synthesis at a value use of a method.

use super::*;

impl SourceTyper<'_> {
    pub(super) fn apply_contextual_arguments(
        &mut self,
        source_tree: TreeId<Untyped>,
        mut function: TreeId<Typed>,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let ty = self.typed_arena.get(function).ty;
        let mut callable = match self.store.types.try_get(ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(method),
                ..
            }) if self.store.symbols.contains(*method)
                && self.store.symbols.get(*method).kind == SymbolKind::Method
                && matches!(self.store.symbols.info(*method), SymbolInfo::Complete(info)
                    if matches!(self.store.types.try_get(*info), Some(Type::Method(m)) if m.kind != MethodKind::Plain)) =>
            {
                self.widen_expression_type_journaled(ty, journal, 0)?
            }
            Some(Type::Method(_)) => ty,
            _ => return Ok(function),
        };
        let position = self.arena.get(source_tree).position;
        let tree_index = source_tree.index();
        let mut visited = HashSet::new();
        while let Some(Type::Method(method)) = self.store.types.try_get(callable) {
            if method.kind == MethodKind::Plain {
                break;
            }
            let method = method.clone();
            if !visited.insert(callable)
                || visited.len() > crate::types::MAX_TYPE_NORMALIZATION_DEPTH
            {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::TooDeep,
                ));
            }
            if self.type_contains_param_ref(method.result, callable)? {
                return Err(TyperError::DependentMethodApplicationDeferred {
                    source: self.source,
                    tree_index,
                    binder: callable,
                    result: method.result,
                });
            }
            let mut arguments = Vec::with_capacity(method.params.len());
            for parameter in method.params {
                if parameter.erased
                    || parameter.varargs
                    || matches!(
                        self.store.types.try_get(parameter.ty),
                        Some(Type::ByName { .. })
                    )
                {
                    return Err(TyperError::UsingApplicationDeferred {
                        source: self.source,
                        tree_index,
                    });
                }
                let symbol = self.contextual_value(parameter.ty, context, tree_index, journal)?;
                let ident = Ident {
                    name: self.store.symbols.get(symbol).name,
                    backquoted: false,
                };
                arguments.push(self.type_resolved_identifier_expression(
                    symbol, ident, position, context, tree_index, journal,
                )?);
            }
            function = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
                function,
                arguments,
                method.result,
                position,
            );
            callable = method.result;
        }
        Ok(function)
    }

    fn contextual_value(
        &mut self,
        expected: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<SymbolId, TyperError> {
        self.validate_expression_scope_stack(context.local_scopes)?;
        let mut scopes = Vec::new();
        let mut stack = context.local_scopes;
        while let Some(id) = stack {
            let frame = &self.expression_scopes[id.index()];
            scopes.push(frame.scope);
            stack = frame.parent;
        }
        for id in self.source_context_chain(context.lexical, tree_index)? {
            let scope = self.index.source_context(id).lexical_scope;
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
        for scope in scopes {
            let symbols = self.contextual_symbols(scope);
            let mut candidates = Vec::new();
            for symbol in symbols {
                let entry = self.store.symbols.get(symbol);
                if self.initializing_local_symbols.contains(&symbol) {
                    continue;
                }
                if !matches!(
                    entry.kind,
                    SymbolKind::Field
                        | SymbolKind::Parameter
                        | SymbolKind::Local
                        | SymbolKind::Value
                ) {
                    return Err(TyperError::UsingApplicationDeferred {
                        source: self.source,
                        tree_index,
                    });
                }
                let reference =
                    self.expression_type_of_symbol(symbol, context.owner, tree_index, journal)?;
                let actual = self.widen_expression_type_journaled(reference, journal, 0)?;
                if self.contextual_conforms(actual, expected, tree_index, journal)? {
                    candidates.push((symbol, actual));
                }
            }
            if let Some(winner) =
                self.choose_contextual_candidate(&candidates, expected, tree_index, journal)?
            {
                return Ok(winner);
            }
        }
        Err(TyperError::MissingContextualArgument {
            source: self.source,
            tree_index,
            expected,
        })
    }

    fn contextual_symbols(&self, scope: ScopeId) -> Vec<SymbolId> {
        let mut symbols: Vec<_> = self
            .store
            .scopes
            .get(scope)
            .entered_symbols()
            .filter(|symbol| {
                let entry = self.store.symbols.get(*symbol);
                entry.name.is_term()
                    && (entry.flags.contains(SymbolFlags::GIVEN)
                        || entry.flags.contains(SymbolFlags::IMPLICIT))
            })
            .collect();
        symbols.sort_by_key(|symbol| {
            let entry = self.store.symbols.get(*symbol);
            (
                entry.position.map(|p| p.span().range().start()),
                self.store.names.resolve(entry.name.text()),
            )
        });
        symbols.dedup();
        symbols
    }

    fn contextual_conforms(
        &mut self,
        actual: TypeId,
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        let mut prepared = HashSet::new();
        self.complete_relation_type(actual, journal, &mut prepared, 0)?;
        self.complete_relation_type(expected, journal, &mut prepared, 0)?;
        self.conforms(actual, expected).map_err(|error| {
            TyperError::ExpectedExpressionConformanceUnsupported {
                source: self.source,
                tree_index,
                actual,
                expected,
                error: Box::new(error),
            }
        })
    }

    fn choose_contextual_candidate(
        &mut self,
        candidates: &[(SymbolId, TypeId)],
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<SymbolId>, TyperError> {
        let mut maxima = Vec::new();
        for &(symbol, actual) in candidates {
            let mut dominated = false;
            for &(other, rival) in candidates {
                if other != symbol
                    && self.contextual_conforms(actual, rival, tree_index, journal)?
                    && !self.contextual_conforms(rival, actual, tree_index, journal)?
                {
                    dominated = true;
                    break;
                }
            }
            if !dominated {
                maxima.push(symbol);
            }
        }
        match maxima.as_slice() {
            [] => Ok(None),
            [winner] => Ok(Some(*winner)),
            _ => Err(TyperError::AmbiguousContextualArgument {
                source: self.source,
                tree_index,
                expected,
                candidates: maxima,
            }),
        }
    }
}
