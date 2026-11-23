//! Replay earlier argument clauses on the selected method's default getter.
use super::*;

struct DefaultCall {
    root: TreeId<Typed>,
    applications: Vec<TreeId<Typed>>,
    getter: SymbolId,
    expected: TypeId,
}

impl SourceTyper<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn contextual_call_arguments(
        &mut self,
        function: &mut TreeId<Typed>,
        binder: TypeId,
        method: &MethodType,
        context: ExpressionContext,
        tree_index: u32,
        mut history: Option<&mut SearchHistory>,
        shared: &mut Vec<TreeId<Typed>>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(Vec<TreeId<Typed>>, ApplyKind), TyperError> {
        let mut args = Vec::with_capacity(method.params.len());
        let mut kind = ApplyKind::Regular;
        for (ordinal, param) in method.params.iter().enumerate() {
            let param = dotty_core::MethodParam {
                ty: self.contextual_parameter_type(binder, param.ty, &args, tree_index)?,
                ..*param
            };
            let default = self.default_call(*function, ordinal, param.ty);
            // A missing candidate may have opened a recursive dictionary before
            // reaching a missing dependency. Its symbols follow the probe's lifetime.
            let saved_dictionary = default
                .as_ref()
                .and_then(|_| history.as_ref().map(|h| h.dictionary.clone()));
            let mut search =
                |typer: &mut Self, nested: &mut Vec<(SymbolId, SymbolInfo)>| match history
                    .as_deref_mut()
                {
                    Some(h) => {
                        typer.contextual_parameter_value(&param, context, tree_index, h, nested)
                    }
                    None => typer.search_contextual_argument(&param, context, tree_index, nested),
                };
            let Some(default) = default else {
                args.push(search(self, journal)?);
                continue;
            };
            let argument = self.semantic_attempt(Some(journal), search);
            match argument {
                Err(TyperError::MissingContextualArgument { .. }) => {
                    if let (Some(h), Some(saved)) = (history.as_deref_mut(), saved_dictionary) {
                        h.dictionary = saved;
                    }
                    kind = ApplyKind::Using;
                    args.push(self.apply_default_getter(
                        default, function, shared, context, tree_index, journal,
                    )?);
                }
                result => args.push(result?),
            }
        }
        Ok((args, kind))
    }

    /// Fill omitted trailing arguments using the selected getter and its earlier clauses.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::typer) fn trailing_ordinary_defaults(
        &mut self,
        function: &mut TreeId<Typed>,
        method: &MethodType,
        supplied: usize,
        context: ExpressionContext,
        tree_index: u32,
        shared: &mut Vec<TreeId<Typed>>,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<TreeId<Typed>>, TyperError> {
        let mut arguments = Vec::new();
        for (ordinal, parameter) in method.params.iter().enumerate().skip(supplied) {
            let call = self.default_call(*function, ordinal, parameter.ty).ok_or(
                TyperError::ApplicationArityMismatch {
                    source: self.source,
                    tree_index,
                    expected: method.params.len(),
                    actual: supplied,
                },
            )?;
            arguments.push(
                self.apply_default_getter(call, function, shared, context, tree_index, journal)?,
            );
        }
        Ok(arguments)
    }

    pub(in crate::typer) fn bind_default_operands(
        &mut self,
        shared: Vec<TreeId<Typed>>,
        expr: TreeId<Typed>,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        if shared.is_empty() {
            return expr;
        }
        let result = self.typed_arena.get(expr).ty;
        TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .block(shared, expr, result, position)
    }

    fn default_call(
        &self,
        mut tree: TreeId<Typed>,
        ordinal: usize,
        expected: TypeId,
    ) -> Option<DefaultCall> {
        let mut applications = Vec::new();
        let mut clause = 0;
        loop {
            if applications.len() >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
                return None;
            }
            let previous = match &self.typed_arena.get(tree).kind {
                TreeKind::Apply(app) => {
                    clause += 1;
                    app.function
                }
                TreeKind::TypeApply(app) => app.function,
                _ => break,
            };
            applications.push(tree);
            tree = previous;
        }
        let method = self
            .store
            .types
            .get(self.typed_arena.get(tree).ty)
            .reference_symbol()?;
        let getter = if matches!(
            self.store.symbols.get(method).origin,
            SymbolOrigin::Tasty(_)
        ) {
            self.binary_default_getter(method, clause, ordinal)?
        } else {
            self.source_default_getter(method, clause, ordinal)?
        };
        applications.reverse();
        Some(DefaultCall {
            root: tree,
            applications,
            getter,
            expected,
        })
    }

    fn source_default_getter(
        &self,
        method: SymbolId,
        clause: usize,
        ordinal: usize,
    ) -> Option<SymbolId> {
        if self.generated_case_class_for_copy(method).is_some() {
            return if clause == 0 {
                self.store.default_getter(method, ordinal)
            } else {
                None
            };
        }
        let SourceDefinition::Canonical {
            source,
            tree: def_tree,
        } = self.index.definition_of(method)?
        else {
            return None;
        };
        if source != self.source {
            return None;
        }
        let TreeKind::DefDef(def) = &self.arena.try_get(def_tree)?.kind else {
            return None;
        };
        let param = *self.context_bound_clauses(def).get(clause)?.get(ordinal)?;
        let getter =
            self.index
                .derived_symbol_at(self.store.symbols.owner(method)?, source, param)?;
        self.contextual_getter_source(getter)?;
        Some(getter)
    }

    /// The TASTy default-getter index counts term parameters across clauses.
    /// Count declaration formals, since a repeated parameter can consume more
    /// than one argument at a call site.
    fn binary_default_getter(
        &self,
        method: SymbolId,
        clause: usize,
        ordinal: usize,
    ) -> Option<SymbolId> {
        let SymbolInfo::Complete(mut signature) = *self.store.symbols.info(method) else {
            return None;
        };
        let mut preceding = 0usize;
        let mut current = 0;
        let mut has_earlier_context = false;
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            match self.store.types.try_get(signature)? {
                Type::Poly(poly) => signature = poly.result,
                Type::Method(parameters) if current == clause => {
                    // The getter may replay the witness across ordinary or empty clauses.
                    if parameters.kind == MethodKind::Plain && !has_earlier_context {
                        return None;
                    }
                    parameters.params.get(ordinal)?;
                    return self
                        .store
                        .default_getter(method, preceding.checked_add(ordinal)?);
                }
                Type::Method(parameters) => {
                    has_earlier_context |= parameters.kind != MethodKind::Plain;
                    preceding = preceding.checked_add(parameters.params.len())?;
                    current += 1;
                    signature = parameters.result;
                }
                _ => return None,
            }
        }
        None
    }

    fn apply_default_getter(
        &mut self,
        call: DefaultCall,
        function: &mut TreeId<Typed>,
        stats: &mut Vec<TreeId<Typed>>,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let source = self.source;
        let deferred = || TyperError::UsingApplicationDeferred { source, tree_index };
        let original = self.typed_arena.get(call.root).clone();
        let method = self
            .store
            .types
            .get(original.ty)
            .reference_symbol()
            .ok_or_else(deferred)?;
        let entry = self.store.symbols.get(call.getter);
        let name = entry.name;
        let binary = matches!(entry.origin, SymbolOrigin::Tasty(_));
        if binary
            && !self.is_selected_member_accessible(
                call.getter,
                entry.owner.ok_or_else(deferred)?,
                context,
                tree_index,
            )?
        {
            return Err(deferred());
        }
        let mut target = call.root;
        let mut getter = match original.kind {
            TreeKind::Select(sel) => {
                let receiver =
                    self.share_default_operand(sel.qualifier, stats, context, journal)?;
                let prefix = self.typed_arena.get(receiver).ty;
                let getter_type = self.store.types.alloc(Type::term_ref(prefix, call.getter));
                if receiver != sel.qualifier {
                    let method_type = self.store.types.alloc(Type::term_ref(prefix, method));
                    target = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                        receiver,
                        sel.name,
                        sel.backquoted,
                        method_type,
                        original.position,
                    );
                }
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                    receiver,
                    name,
                    false,
                    getter_type,
                    original.position,
                )
            }
            TreeKind::Ident(_) => self.type_resolved_identifier_expression(
                call.getter,
                Ident {
                    name,
                    backquoted: false,
                },
                original.position,
                context,
                tree_index,
                journal,
            )?,
            _ => return Err(deferred()),
        };
        let mut signature =
            self.widen_expression_type_journaled(self.typed_arena.get(getter).ty, journal, 0)?;
        for tree in call.applications {
            let node = self.typed_arena.get(tree).clone();
            match node.kind {
                TreeKind::TypeApply(app) => {
                    let args = app
                        .args
                        .iter()
                        .map(|t| self.typed_arena.get(*t).ty)
                        .collect::<Vec<_>>();
                    signature = dotty_core::types::instantiate_poly(self.store, signature, &args)
                        .map_err(TyperError::PolyInstantiation)?
                        .result;
                    let mut b = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
                    if target != app.function {
                        target = b.type_apply(target, app.args.clone(), node.ty, node.position)
                    } else {
                        target = tree
                    }
                    getter = b.type_apply(getter, app.args, signature, node.position);
                }
                TreeKind::Apply(app) => {
                    let Some(Type::Method(previous)) = self.store.types.try_get(signature).cloned()
                    else {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: self.source,
                            tree_index,
                        });
                    };
                    let dependent = self.type_contains_param_ref(previous.result, signature)?;
                    if previous.params.len() != app.args.len()
                        || (dependent && (binary || previous.kind == MethodKind::Plain))
                    {
                        return Err(TyperError::UsingApplicationDeferred {
                            source: self.source,
                            tree_index,
                        });
                    }
                    if binary
                        && !self.binary_getter_clause_matches(
                            app.function,
                            &previous,
                            tree_index,
                            journal,
                        )?
                    {
                        return Err(deferred());
                    }
                    let mut shared = Vec::with_capacity(app.args.len());
                    for (&arg, param) in app.args.iter().zip(previous.params) {
                        shared.push(
                            if matches!(self.store.types.get(param.ty), Type::ByName { .. }) {
                                arg
                            } else {
                                self.share_default_operand(arg, stats, context, journal)?
                            },
                        );
                    }
                    signature = if dependent {
                        // A widened temporary cannot stand for the original singleton path.
                        let paths = shared
                            .iter()
                            .zip(&app.args)
                            .map(|(&copy, &arg)| (copy == arg).then_some(arg));
                        self.instantiate_contextual_component(
                            signature,
                            MethodComponent::Result(previous.result),
                            paths,
                            tree_index,
                        )?
                    } else {
                        previous.result
                    };
                    let mut b = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
                    if target != app.function || shared != app.args {
                        target = b.apply_with_kind(
                            target,
                            shared.clone(),
                            app.kind,
                            node.ty,
                            node.position,
                        )
                    } else {
                        target = tree
                    }
                    getter = b.apply_with_kind(getter, shared, app.kind, signature, node.position);
                }
                _ => {
                    return Err(TyperError::UsingApplicationDeferred {
                        source: self.source,
                        tree_index,
                    });
                }
            }
        }
        if binary || self.copy_default_field(call.getter).is_some() {
            // A parameterless binary getter has ExprType and is selected without ().
            if let Some(Type::ByName { result }) = self.store.types.try_get(signature) {
                signature = *result;
            }
            let expected = match self.store.types.try_get(call.expected) {
                Some(Type::ByName { result }) => *result,
                _ => call.expected,
            };
            if !self.contextual_conforms(signature, expected, tree_index, journal)? {
                return Err(TyperError::ExpectedExpressionTypeMismatch {
                    source: self.source,
                    tree_index,
                    actual: signature,
                    expected,
                });
            }
        }
        *function = target;
        Ok(getter)
    }

    /// Both the receiver and earlier arguments will be used with two distinct
    /// declarations. Authenticate the copied clause before rebuilding those calls.
    fn binary_getter_clause_matches(
        &mut self,
        target: TreeId<Typed>,
        copied: &MethodType,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        let applied =
            self.widen_expression_type_journaled(self.typed_arena.get(target).ty, journal, 0)?;
        let Some(Type::Method(original)) = self.store.types.try_get(applied).cloned() else {
            return Ok(false);
        };
        if original.kind != copied.kind || original.params.len() != copied.params.len() {
            return Ok(false);
        }
        for (param, copy) in original.params.iter().zip(&copied.params) {
            if param.erased != copy.erased
                || param.varargs != copy.varargs
                || !self.contextual_conforms(param.ty, copy.ty, tree_index, journal)?
                || !self.contextual_conforms(copy.ty, param.ty, tree_index, journal)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn share_default_operand(
        &mut self,
        tree: TreeId<Typed>,
        stats: &mut Vec<TreeId<Typed>>,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let node = self.typed_arena.get(tree);
        let ty = node.ty;
        if matches!(node.kind, TreeKind::This(_) | TreeKind::Literal(_)) {
            return Ok(tree);
        }
        if matches!(&node.kind, TreeKind::Ident(_))
            && let Some(id) = self.store.types.get(ty).reference_symbol()
            && self.store.symbols.contains(id)
        {
            let s = self.store.symbols.get(id);
            if matches!(
                s.kind,
                SymbolKind::Parameter | SymbolKind::Local | SymbolKind::Object
            ) && !s.flags.contains(SymbolFlags::MUTABLE)
                && !s.flags.contains(SymbolFlags::LAZY)
                && !matches!(s.info,SymbolInfo::Complete(t) if matches!(self.store.types.try_get(t),Some(Type::ByName{..})))
            {
                return Ok(tree);
            }
        }
        let position = node.position;
        let value_type = self.widen_expression_type_journaled(ty, journal, 0)?;
        let name = dotty_core::TermName::new(self.store.names.intern("defaultArg$"));
        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name: *name.as_name(),
            owner: Some(context.owner),
            kind: SymbolKind::Local,
            flags: SymbolFlags::SYNTHETIC,
            visibility: Visibility::Private,
            info: SymbolInfo::Complete(value_type),
            origin: SymbolOrigin::Synthetic,
            position,
            annotations: Vec::new(),
            links: Default::default(),
        });
        let tpt = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(value_type, None);
        let definition = self.typed_index.alloc_synthetic_definition(
            &mut self.typed_arena,
            symbol,
            Tree {
                ty: value_type,
                position,
                kind: TreeKind::ValDef(dotty_core::ast::ValDef {
                    name,
                    tpt,
                    rhs: Some(tree),
                    metadata: (),
                }),
            },
        );
        stats.push(definition);
        let reference = self
            .store
            .types
            .alloc(Type::term_ref(self.definitions.no_prefix, symbol));
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).ident(
                *name.as_name(),
                reference,
                None,
            ),
        )
    }
}
