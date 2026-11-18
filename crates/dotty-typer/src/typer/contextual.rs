//! Context argument synthesis at a value use of a method.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContextualRef {
    symbol: SymbolId,
    receiver: Option<SymbolId>,
}

#[derive(Clone)]
struct Provider {
    reference: ContextualRef,
    type_arguments: Vec<TypeId>,
    callable: TypeId,
    result: TypeId,
}

#[derive(Default)]
struct SearchHistory {
    active: Vec<(ContextualRef, TypeId)>,
    attempts: usize,
    divergences: usize,
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
                arguments.push(self.contextual_value(
                    parameter.ty,
                    context,
                    tree_index,
                    &mut SearchHistory::default(),
                    journal,
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
        history: &mut SearchHistory,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let divergences = history.divergences;
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
            if let Some(winner) =
                self.choose_contextual_refs(refs, expected, context, tree_index, history, journal)?
            {
                return Ok(winner);
            }
        }
        let companions = self.companion_refs(expected, tree_index, journal)?;
        if let Some(winner) = self
            .choose_contextual_refs(companions, expected, context, tree_index, history, journal)?
        {
            return Ok(winner);
        }
        if history.divergences > divergences {
            return Err(TyperError::DivergentContextualArgument {
                source: self.source,
                tree_index,
                expected,
            });
        }
        Err(TyperError::MissingContextualArgument {
            source: self.source,
            tree_index,
            expected,
        })
    }

    fn choose_contextual_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        expected: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        history: &mut SearchHistory,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
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
                    | SymbolKind::Method
                    | SymbolKind::Object
            ) {
                return Err(TyperError::UsingApplicationDeferred {
                    source: self.source,
                    tree_index,
                });
            }
            let reference = if let Some(receiver) = evidence.receiver {
                let prefix =
                    self.expression_type_of_symbol(receiver, context.owner, tree_index, journal)?;
                self.store.types.alloc(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(symbol),
                })
            } else {
                self.expression_type_of_symbol(symbol, context.owner, tree_index, journal)?
            };
            let actual = self.widen_expression_type_journaled(reference, journal, 0)?;
            if let Some(provider) =
                self.contextual_provider(evidence, actual, expected, tree_index, journal)?
            {
                candidates.push(provider);
            }
        }
        if let [provider] = candidates.as_slice() {
            if !matches!(
                self.store.types.try_get(provider.callable),
                Some(Type::Method(_))
            ) {
                return self
                    .materialize_provider(provider, expected, context, tree_index, history, journal)
                    .map(Some);
            }
            return match self.semantic_attempt(Some(journal), |typer, nested| {
                typer.materialize_provider(provider, expected, context, tree_index, history, nested)
            }) {
                Err(
                    TyperError::MissingContextualArgument { .. }
                    | TyperError::DivergentContextualArgument { .. },
                ) => Ok(None),
                other => other.map(Some),
            };
        }
        let mut eligible = Vec::new();
        for provider in candidates {
            if matches!(
                self.store.types.try_get(provider.callable),
                Some(Type::Method(_))
            ) {
                let attempt = self.probe_contextual_candidate(|typer, probe_journal| {
                    typer
                        .materialize_provider(
                            &provider,
                            expected,
                            context,
                            tree_index,
                            history,
                            probe_journal,
                        )
                        .map(|_| ())
                });
                match attempt {
                    Err(
                        TyperError::MissingContextualArgument { .. }
                        | TyperError::DivergentContextualArgument { .. },
                    ) => continue,
                    other => other?,
                }
            }
            eligible.push(provider);
        }
        let Some(provider) =
            self.choose_contextual_candidate(&eligible, expected, tree_index, journal)?
        else {
            return Ok(None);
        };
        self.materialize_provider(provider, expected, context, tree_index, history, journal)
            .map(Some)
    }

    fn contextual_provider(
        &mut self,
        reference: ContextualRef,
        signature: TypeId,
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<Provider>, TyperError> {
        let poly = match self.store.types.try_get(signature) {
            Some(Type::Poly(poly)) => Some(poly.clone()),
            _ => None,
        };
        let mut result = poly.as_ref().map_or(signature, |poly| poly.result);
        let mut depth = 0;
        while let Some(Type::Method(method)) = self.store.types.try_get(result) {
            if method.kind == MethodKind::Plain {
                return Ok(None);
            }
            if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH
                || method.params.iter().any(|p| {
                    p.erased
                        || p.varargs
                        || matches!(self.store.types.try_get(p.ty), Some(Type::ByName { .. }))
                })
            {
                return Err(TyperError::UsingApplicationDeferred {
                    source: self.source,
                    tree_index,
                });
            }
            depth += 1;
            result = method.result;
        }
        let mut callable = signature;
        let mut type_arguments = Vec::new();
        if let Some(poly) = poly {
            let mut inferred = vec![None; poly.params.len()];
            match self.infer_type_constraints(
                result,
                expected,
                signature,
                &mut inferred,
                InferenceLocation {
                    tree_index,
                    argument_index: 0,
                    depth: 0,
                },
                journal,
            ) {
                Ok(()) => {}
                Err(
                    TyperError::UnsupportedInferenceShape { .. }
                    | TyperError::ConflictingInferenceConstraints { .. },
                ) => return Ok(None),
                Err(error) => return Err(error),
            }
            let Some(arguments) = inferred.into_iter().collect::<Option<Vec<_>>>() else {
                return Err(TyperError::UsingApplicationDeferred {
                    source: self.source,
                    tree_index,
                });
            };
            type_arguments = arguments;
            let instance =
                dotty_core::types::instantiate_poly(self.store, signature, &type_arguments)
                    .map_err(TyperError::PolyInstantiation)?;
            for (index, (&argument, bounds)) in
                type_arguments.iter().zip(instance.bounds).enumerate()
            {
                match self.check_explicit_type_argument_bounds(
                    argument, bounds, index, tree_index, journal,
                ) {
                    Ok(()) => {}
                    Err(TyperError::ExplicitTypeArgumentBoundViolation { .. }) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            callable = instance.result;
            result = callable;
            for _ in 0..depth {
                let Some(Type::Method(method)) = self.store.types.try_get(result) else {
                    return Err(TyperError::UsingApplicationDeferred {
                        source: self.source,
                        tree_index,
                    });
                };
                result = method.result;
            }
        }
        if !self.contextual_conforms(result, expected, tree_index, journal)? {
            return Ok(None);
        }
        Ok(Some(Provider {
            reference,
            type_arguments,
            callable,
            result,
        }))
    }

    fn materialize_provider(
        &mut self,
        provider: &Provider,
        expected: TypeId,
        context: ExpressionContext,
        tree_index: u32,
        history: &mut SearchHistory,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        history.attempts += 1;
        if history.attempts > 4096 || history.active.len() >= 32 {
            return Err(TyperError::ContextualSearchLimit {
                source: self.source,
                tree_index,
            });
        }
        for &(reference, ancestor) in &history.active {
            if reference == provider.reference
                && self.contextual_conforms(expected, ancestor, tree_index, journal)?
                && self.contextual_conforms(ancestor, expected, tree_index, journal)?
            {
                history.divergences += 1;
                return Err(TyperError::DivergentContextualArgument {
                    source: self.source,
                    tree_index,
                    expected,
                });
            }
        }
        history.active.push((provider.reference, expected));
        let result = self.build_provider(provider, context, tree_index, history, journal);
        history.active.pop();
        result
    }

    fn build_provider(
        &mut self,
        provider: &Provider,
        context: ExpressionContext,
        tree_index: u32,
        history: &mut SearchHistory,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let mut tree =
            self.contextual_reference_tree(provider.reference, context, tree_index, None, journal)?;
        let mut signature = provider.callable;
        if !provider.type_arguments.is_empty() {
            let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
            let args = provider
                .type_arguments
                .iter()
                .map(|ty| builder.type_tree(*ty, None))
                .collect();
            tree = builder.type_apply(tree, args, signature, None);
        }
        while let Some(Type::Method(method)) = self.store.types.try_get(signature).cloned() {
            if self.type_contains_param_ref(method.result, signature)? {
                return Err(TyperError::DependentMethodApplicationDeferred {
                    source: self.source,
                    tree_index,
                    binder: signature,
                    result: method.result,
                });
            }
            let mut args = Vec::new();
            for param in method.params {
                args.push(self.contextual_value(param.ty, context, tree_index, history, journal)?);
            }
            tree = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
                tree,
                args,
                method.result,
                None,
            );
            signature = method.result;
        }
        Ok(tree)
    }

    /// Collect nominal anchors and their parent scopes only after lexical search
    /// has no winner. Each companion contributes a stable term prefix.
    fn companion_refs(
        &mut self,
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<ContextualRef>, TyperError> {
        let mut pending = vec![expected];
        let mut seen = HashSet::new();
        let mut classes = HashSet::new();
        let mut refs = Vec::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            if seen.len() > MAX_TYPE_RELATION_VIEWS {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::TooDeep,
                ));
            }
            match self.store.types.try_get(ty).cloned() {
                Some(Type::Applied { tycon, args }) => {
                    pending.extend(args);
                    pending.push(tycon);
                }
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(class),
                    ..
                }) if self.store.symbols.contains(class) => {
                    let entry = self.store.symbols.get(class);
                    if !matches!(
                        entry.kind,
                        SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                    ) || !classes.insert(class)
                    {
                        continue;
                    }
                    if matches!(entry.origin, SymbolOrigin::Builtin) {
                        continue;
                    }
                    let info = self
                        .class_info(class, journal)
                        .map_err(|e| TyperError::MemberLookup(Box::new(e)))?;
                    pending.extend(info.parents);
                    let entry = self.store.symbols.get(class);
                    if entry.links.companion.is_none()
                        && matches!(entry.origin, SymbolOrigin::Tasty(_))
                        && let Some(package) = entry.owner
                        && self.store.symbols.get(package).kind == SymbolKind::Package
                    {
                        // Loading the type alone may leave the object unentered.
                        // Only the resolver can establish their canonical link.
                        let name =
                            dotty_core::Name::new(entry.name.text(), dotty_core::Namespace::Term);
                        let request = MemberRequest {
                            name,
                            prefix: self.package_type_prefix(package),
                            selector: MemberSelector::Unique,
                            space: MemberSpace::Prefix,
                        };
                        self.resolver
                            .resolve_member(self.store, &request)
                            .map_err(|error| TyperError::SymbolResolution {
                                source: self.source,
                                tree_index,
                                error,
                            })?;
                    }
                    let Some(object) = self.store.symbols.get(class).links.companion else {
                        continue;
                    };
                    if !self.store.symbols.contains(object)
                        || self.store.symbols.get(object).kind != SymbolKind::Object
                        || self.store.symbols.get(object).links.companion != Some(class)
                    {
                        return Err(TyperError::ObjectModuleClassUnavailable { object });
                    }
                    let module = self.module_class_of_object(object)?;
                    let scope = self
                        .class_info(module, journal)
                        .map_err(|e| TyperError::MemberLookup(Box::new(e)))?
                        .declarations;
                    for symbol in self.contextual_symbols(scope) {
                        if self.store.symbols.get(symbol).visibility == Visibility::Public {
                            let evidence = ContextualRef {
                                symbol,
                                receiver: Some(object),
                            };
                            if !refs.contains(&evidence) {
                                refs.push(evidence);
                            }
                        }
                    }
                }
                Some(Type::And { left, right } | Type::Or { left, right }) => {
                    pending.extend([left, right])
                }
                Some(Type::AliasingBounds { alias }) => pending.push(alias),
                _ => {}
            }
        }
        Ok(refs)
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

    fn choose_contextual_candidate<'p>(
        &mut self,
        candidates: &'p [Provider],
        expected: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<&'p Provider>, TyperError> {
        let mut maxima = Vec::new();
        for candidate in candidates {
            let mut dominated = false;
            for rival in candidates {
                if rival.reference == candidate.reference {
                    continue;
                }
                let is_method = |p: &Provider| {
                    !p.type_arguments.is_empty()
                        || matches!(self.store.types.get(p.callable), Type::Method(_))
                };
                let candidate_method = is_method(candidate);
                let rival_method = is_method(rival);
                if candidate_method != rival_method {
                    dominated = candidate_method;
                } else {
                    let legacy = |p: &Provider| {
                        self.store
                            .symbols
                            .get(p.reference.symbol)
                            .flags
                            .contains(SymbolFlags::IMPLICIT)
                    };
                    let candidate_legacy = legacy(candidate);
                    let rival_legacy = legacy(rival);
                    let narrower = self.contextual_conforms(
                        candidate.result,
                        rival.result,
                        tree_index,
                        journal,
                    )?;
                    let broader = self.contextual_conforms(
                        rival.result,
                        candidate.result,
                        tree_index,
                        journal,
                    )?;
                    // Scala 3.9 compares a given in the general direction, and an
                    // old-style implicit in the specific direction.
                    let candidate_as_good = if candidate_legacy { narrower } else { broader };
                    let rival_as_good = if rival_legacy { broader } else { narrower };
                    dominated = rival_as_good && !candidate_as_good;
                }
                if dominated {
                    break;
                }
            }
            if !dominated {
                maxima.push(candidate);
            }
        }
        match maxima.as_slice() {
            [] => Ok(None),
            [winner] => Ok(Some(*winner)),
            _ => Err(TyperError::AmbiguousContextualArgument {
                source: self.source,
                tree_index,
                expected,
                candidates: maxima.iter().map(|p| p.reference.symbol).collect(),
            }),
        }
    }
}
