//! Context argument synthesis at a value use of a method.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContextualRef {
    symbol: SymbolId,
    receiver: Option<SymbolId>,
}

struct ContextualScope {
    scope: ScopeId,
    imports: Vec<(TreeId<Untyped>, SourceContextId)>,
}

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
                let evidence = self.contextual_value(parameter.ty, context, tree_index, journal)?;
                arguments.push(
                    self.contextual_reference_tree(
                        evidence, context, tree_index, position, journal,
                    )?,
                );
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

    fn contextual_reference_tree(
        &mut self,
        evidence: ContextualRef,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let name = self.store.symbols.get(evidence.symbol).name;
        if let Some(receiver) = evidence.receiver {
            let ident = Ident {
                name: self.store.symbols.get(receiver).name,
                backquoted: false,
            };
            let qualifier = self.type_resolved_identifier_expression(
                receiver, ident, position, context, tree_index, journal,
            )?;
            let ty = self.store.types.alloc(Type::TermRef {
                prefix: self.typed_arena.get(qualifier).ty,
                target: TermRefTarget::Symbol(evidence.symbol),
            });
            return Ok(
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .select(qualifier, name, false, ty, position),
            );
        }
        self.type_resolved_identifier_expression(
            evidence.symbol,
            Ident {
                name,
                backquoted: false,
            },
            position,
            context,
            tree_index,
            journal,
        )
    }

    fn contextual_value(
        &mut self,
        expected: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ContextualRef, TyperError> {
        self.validate_expression_scope_stack(context.local_scopes)?;
        let mut scopes = Vec::new();
        let mut stack = context.local_scopes;
        while let Some(id) = stack {
            let frame = &self.expression_scopes[id.index()];
            scopes.push(ContextualScope {
                scope: frame.scope,
                imports: frame
                    .imports
                    .iter()
                    .map(|(tree, ctx)| (*tree, ctx.lexical))
                    .collect(),
            });
            stack = frame.parent;
        }
        for id in self.source_context_chain(context.lexical, tree_index)? {
            let lexical = self.index.source_context(id);
            let imports = if let Some(level) = scopes
                .iter_mut()
                .find(|level| level.scope == lexical.lexical_scope)
            {
                &mut level.imports
            } else {
                scopes.push(ContextualScope {
                    scope: lexical.lexical_scope,
                    imports: Vec::new(),
                });
                &mut scopes.last_mut().unwrap().imports
            };
            if let Some(tree) = lexical.import {
                imports.insert(0, (tree, lexical.parent.unwrap_or(id)));
            }
        }
        for ContextualScope { scope, imports } in scopes {
            let mut refs: Vec<_> = self
                .contextual_symbols(scope)
                .into_iter()
                .map(|symbol| ContextualRef {
                    symbol,
                    receiver: None,
                })
                .collect();
            for (import, lexical) in imports {
                self.contextual_import_refs(
                    import, lexical, context, tree_index, journal, &mut refs,
                )?;
            }
            let mut candidates = Vec::new();
            for evidence in refs {
                let symbol = evidence.symbol;
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
                let reference = if let Some(receiver) = evidence.receiver {
                    let prefix = self.expression_type_of_symbol(
                        receiver,
                        context.owner,
                        tree_index,
                        journal,
                    )?;
                    self.store.types.alloc(Type::TermRef {
                        prefix,
                        target: TermRefTarget::Symbol(symbol),
                    })
                } else {
                    self.expression_type_of_symbol(symbol, context.owner, tree_index, journal)?
                };
                let actual = self.widen_expression_type_journaled(reference, journal, 0)?;
                if self.contextual_conforms(actual, expected, tree_index, journal)? {
                    candidates.push((evidence, actual));
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

    fn contextual_import_refs(
        &mut self,
        tree: TreeId<Untyped>,
        lexical: SourceContextId,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
        refs: &mut Vec<ContextualRef>,
    ) -> Result<(), TyperError> {
        let Some(TreeKind::Import(import)) = self.arena.try_get(tree).map(|node| &node.kind) else {
            return Err(TyperError::MalformedSourceImport {
                source: self.source,
                import_tree_index: tree.index(),
            });
        };
        let import = import.clone();
        let receiver = match self
            .typed_index
            .get(self.source, import.expr)
            .and_then(|qualifier| self.store.types.try_get(self.typed_arena.get(qualifier).ty))
        {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(receiver),
                ..
            }) => Some(*receiver),
            _ => self.import_qualifier_symbol(
                import.expr,
                lexical,
                tree.index(),
                self.arena.get(tree).position,
            )?,
        }
        .ok_or(TyperError::ImportQualifierNotFound {
            source: self.source,
            import_tree_index: tree.index(),
        })?;
        let kind = self.store.symbols.get(receiver).kind;
        let qualified = match kind {
            SymbolKind::Package => None,
            SymbolKind::Object
            | SymbolKind::Local
            | SymbolKind::Value
            | SymbolKind::Parameter
            | SymbolKind::Field => Some(receiver),
            _ => return Ok(()),
        };
        let mut scopes = self.scopes_of(receiver);
        if let Some(receiver) = qualified {
            let prefix =
                self.expression_type_of_symbol(receiver, context.owner, tree_index, journal)?;
            self.require_stable_selection_prefix(prefix, tree_index)?;
            let ty = self.widen_expression_type_journaled(prefix, journal, 0)?;
            self.complete_relation_type(ty, journal, &mut HashSet::new(), 0)?;
            let tycon = match self.store.types.try_get(ty) {
                Some(Type::Applied { tycon, .. }) => *tycon,
                _ => ty,
            };
            if let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) = self.store.types.try_get(tycon)
            {
                for scope in self.scopes_of(*class) {
                    if !scopes.contains(&scope) {
                        scopes.push(scope);
                    }
                }
            }
        }
        for scope in scopes {
            for symbol in self.contextual_symbols(scope) {
                let entry = self.store.symbols.get(symbol);
                if entry.visibility != Visibility::Public {
                    continue;
                }
                let name = entry.name;
                let legacy = entry.flags.contains(SymbolFlags::IMPLICIT);
                let selected_by_name = import
                    .selectors
                    .iter()
                    .find(|s| s.imported.text() == name.text());
                let selected = if let Some(named) = selected_by_name {
                    !named.renamed.is_some_and(|rename| matches!(&self.arena.get(rename).kind, TreeKind::Ident(id) if !id.backquoted && self.store.names.resolve(id.name.text()) == "_"))
                } else {
                    let mut matched = false;
                    for selector in &import.selectors {
                        match self.store.names.resolve(selector.imported.text()) {
                            "*" if legacy && !selector.imported_backquoted => matched = true,
                            "" => {
                                if let Some(bound) = selector.bound {
                                    let bound =
                                        self.type_of_tpt_inner_journaled(bound, lexical, journal)?;
                                    let actual =
                                        self.completed_expression_symbol_info(symbol, journal)?;
                                    matched |= self
                                        .contextual_conforms(actual, bound, tree_index, journal)?;
                                } else {
                                    matched = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    matched
                };
                let evidence = ContextualRef {
                    symbol,
                    receiver: qualified,
                };
                if selected {
                    // Repeated imports of the same declaration select its latest receiver.
                    refs.retain(|prior| prior.symbol != symbol || prior.receiver.is_none());
                    if !refs.contains(&evidence) {
                        refs.push(evidence);
                    }
                }
            }
        }
        Ok(())
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
        candidates: &[(ContextualRef, TypeId)],
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ContextualRef>, TyperError> {
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
                candidates: maxima.iter().map(|r| r.symbol).collect(),
            }),
        }
    }
}
