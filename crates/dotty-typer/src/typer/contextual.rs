//! Context argument synthesis at a value use of a method.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContextualRef {
    symbol: SymbolId,
    receiver: Option<ContextualReceiver>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextualReceiver {
    Companion(SymbolId),
    Import(TreeId<Typed>),
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
        function: TreeId<Typed>,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let ty = self.typed_arena.get(function).ty;
        let callable = match self.store.types.try_get(ty) {
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
        let applied =
            self.supply_contextual_clauses(source_tree, function, callable, context, journal)?;
        let ty = self.typed_arena.get(applied).ty;
        if let Some(Type::Method(method)) = self.store.types.try_get(ty)
            && method.kind == MethodKind::Plain
            && method.params.is_empty()
            && self.is_contextual_constructor_chain(applied)
        {
            let result = method.result;
            return Ok(
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
                    applied,
                    Vec::new(),
                    result,
                    self.arena.get(source_tree).position,
                ),
            );
        }
        Ok(applied)
    }

    fn is_contextual_constructor_chain(&self, mut tree: TreeId<Typed>) -> bool {
        let mut has_context = false;
        while let TreeKind::Apply(app) = &self.typed_arena.get(tree).kind {
            tree = app.function;
            let ty = self.typed_arena.get(tree).ty;
            let signature = match self.store.types.try_get(ty) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(method),
                    ..
                }) if self.store.symbols.contains(*method) => {
                    let entry = self.store.symbols.get(*method);
                    if entry.kind != SymbolKind::Constructor {
                        return false;
                    }
                    let mut signature = entry.info;
                    if let SymbolInfo::Complete(poly) = signature
                        && let Some(Type::Poly(poly)) = self.store.types.try_get(poly)
                    {
                        signature = SymbolInfo::Complete(poly.result);
                    }
                    return has_context
                        || matches!(signature,SymbolInfo::Complete(id) if matches!(self.store.types.try_get(id),Some(Type::Method(m)) if m.kind!=MethodKind::Plain));
                }
                other => other,
            };
            has_context |= matches!(signature,Some(Type::Method(m)) if m.kind!=MethodKind::Plain);
        }
        false
    }

    pub(super) fn supply_contextual_clauses(
        &mut self,
        source_tree: TreeId<Untyped>,
        mut function: TreeId<Typed>,
        mut callable: TypeId,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
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
            function = self.finish_contextual_call(
                function,
                callable,
                method.result,
                arguments,
                ApplyKind::Regular,
                position,
                tree_index,
            )?;
            callable = self.typed_arena.get(function).ty;
        }
        Ok(function)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_contextual_call(
        &mut self,
        function: TreeId<Typed>,
        binder: TypeId,
        result: TypeId,
        args: Vec<TreeId<Typed>>,
        kind: ApplyKind,
        position: Option<SourceSpan>,
        tree_index: u32,
    ) -> Result<TreeId<Typed>, TyperError> {
        let deferred = TyperError::DependentMethodApplicationDeferred {
            source: self.source,
            tree_index,
            binder,
            result,
        };
        let result = if let Some(Type::ParamRef {
            binder: owner,
            index,
        }) = self.store.types.try_get(result)
            && *owner == binder
        {
            let Some(&arg) = args.get(*index as usize) else {
                return Err(deferred);
            };
            let arg_ty = self.typed_arena.get(arg).ty;
            if args.len() == 1
                && let TreeKind::TypeApply(tapp) = &self.typed_arena.get(function).kind
                && let Some(Type::TermRef {
                    target: TermRefTarget::Symbol(method),
                    ..
                }) = self
                    .store
                    .types
                    .try_get(self.typed_arena.get(tapp.function).ty)
                    .cloned()
                && self.store.symbols.contains(method)
                && self
                    .store
                    .symbols
                    .get(method)
                    .flags
                    .contains(SymbolFlags::TRANSPARENT | SymbolFlags::INLINE)
                && self
                    .store
                    .names
                    .resolve(self.store.symbols.get(method).name.text())
                    == "summon"
                && matches!(
                    self.store.symbols.get(method).origin,
                    SymbolOrigin::Tasty(_)
                )
                && self
                    .predef_context_member(self.store.symbols.get(method).name, tree_index)?
                    .is_some_and(|(_, symbol)| symbol == method)
            {
                return Ok(arg);
            }
            self.require_stable_selection_prefix(arg_ty, tree_index)
                .map_err(|_| deferred)?;
            arg_ty
        } else if self.type_contains_param_ref(result, binder)? {
            return Err(deferred);
        } else {
            result
        };
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .apply_with_kind(function, args, kind, result, position),
        )
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
            let qualifier = match receiver {
                ContextualReceiver::Import(tree) => tree,
                ContextualReceiver::Companion(object) => {
                    let ident = Ident {
                        name: self.store.symbols.get(object).name,
                        backquoted: false,
                    };
                    self.type_resolved_identifier_expression(
                        object, ident, position, context, tree_index, journal,
                    )?
                }
            };
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
                .contextual_members(scope, journal)?
                .into_iter()
                .map(|symbol| ContextualRef {
                    symbol,
                    receiver: None,
                })
                .collect();
            for (import, lexical) in imports {
                self.contextual_import_refs(import, lexical, tree_index, journal, &mut refs)?;
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
                let prefix = match receiver {
                    ContextualReceiver::Import(tree) => self.typed_arena.get(tree).ty,
                    ContextualReceiver::Companion(object) => {
                        self.expression_type_of_symbol(object, context.owner, tree_index, journal)?
                    }
                };
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
                    for symbol in self.contextual_members(scope, journal)? {
                        if self.store.symbols.get(symbol).visibility == Visibility::Public {
                            let evidence = ContextualRef {
                                symbol,
                                receiver: Some(ContextualReceiver::Companion(object)),
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
        let qualified = match self.store.symbols.get(receiver).kind {
            SymbolKind::Package => None,
            SymbolKind::Object
            | SymbolKind::Local
            | SymbolKind::Value
            | SymbolKind::Parameter
            | SymbolKind::Field => {
                // A block import already has its typed binding. A member import
                // belongs to the lexical context at its declaration, even if
                // the use site contains a shadowing local of the same name.
                let import_context = ExpressionContext {
                    lexical,
                    owner: self.index.source_context(lexical).owner,
                    local_scopes: None,
                };
                Some(self.type_import_qualifier(
                    import.expr,
                    import_context,
                    tree.index(),
                    journal,
                    &mut Vec::new(),
                )?)
            }
            _ => return Ok(()),
        };
        let mut scopes = self.scopes_of(receiver);
        if let Some(qualifier) = qualified {
            let prefix = self.typed_arena.get(qualifier).ty;
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
            for symbol in self.contextual_members(scope, journal)? {
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
                    receiver: qualified.map(ContextualReceiver::Import),
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

    fn contextual_members(
        &mut self,
        scope: ScopeId,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let mut symbols = self.contextual_symbols(scope);
        let Some(class) = self.store.scopes.get(scope).owner else {
            return Ok(symbols);
        };
        if !matches!(
            self.store.symbols.get(class).kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return Ok(symbols);
        }
        let error = |e| TyperError::MemberLookup(Box::new(e));
        let receiver = self.store.types.alloc(Type::ThisType { class });
        let mut pending = self.class_info(class, journal).map_err(error)?.parents;
        let mut seen = HashSet::from([class]);
        let mut names = HashSet::new();
        // Enumerate names on ancestors, then use ordinary member lookup so an
        // overriding non-given member can hide its inherited namesake.
        while let Some(parent) = pending.pop() {
            let parent =
                super::lookup::class_symbol_for_type(self.store, parent, true).map_err(error)?;
            if !seen.insert(parent) || parent == self.definitions.object_class {
                continue;
            }
            if seen.len() > MAX_TYPE_RELATION_VIEWS {
                return Err(error(MemberLookupError::TooDeep));
            }
            let info = self.class_info(parent, journal).map_err(error)?;
            for symbol in self.contextual_symbols(info.declarations) {
                names.insert(self.store.symbols.get(symbol).name);
            }
            pending.extend(info.parents);
        }
        let mut names: Vec<_> = names.into_iter().collect();
        names.sort_by_key(|name| self.store.names.resolve(name.text()));
        for name in names {
            for candidate in self
                .lookup_members_journaled(receiver, name, journal)
                .map_err(error)?
            {
                let entry = self.store.symbols.get(candidate.symbol);
                if candidate.declaring_class != class
                    && entry.visibility == Visibility::Public
                    && (entry.flags.contains(SymbolFlags::GIVEN)
                        || entry.flags.contains(SymbolFlags::IMPLICIT))
                    && !symbols.contains(&candidate.symbol)
                {
                    symbols.push(candidate.symbol);
                }
            }
        }
        Ok(symbols)
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
