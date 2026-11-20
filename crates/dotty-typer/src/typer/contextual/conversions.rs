//! Selects either a Conversion instance or an old-style unary implicit method.

use super::*;

pub(in crate::typer) enum ConversionTarget {
    Instance(TreeId<Typed>),
    Method {
        function: TreeId<Typed>,
        signature: TypeId,
    },
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct ConversionGoal {
    pub(in crate::typer) instance: Option<TypeId>,
    pub(in crate::typer) actual: TypeId,
    pub(in crate::typer) expected: TypeId,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn conversion_target(
        &mut self,
        goal: ConversionGoal,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ConversionTarget>, TyperError> {
        for level in self.contextual_scopes(context, tree_index)? {
            let refs = self.contextual_scope_refs(level, tree_index, journal)?;
            if let Some(target) =
                self.conversion_from_refs(refs, goal, context, tree_index, journal)?
            {
                return Ok(Some(target));
            }
        }
        let anchors: &[TypeId] = match &goal.instance {
            Some(instance) => std::slice::from_ref(instance),
            None => &[goal.actual, goal.expected],
        };
        let mut refs = Vec::new();
        for &anchor in anchors {
            let Some(candidates) = self.adaptation_companion_refs(anchor, tree_index, journal)?
            else {
                return Ok(None);
            };
            for candidate in candidates {
                if !refs.contains(&candidate) {
                    refs.push(candidate);
                }
            }
        }
        self.conversion_from_refs(refs, goal, context, tree_index, journal)
    }

    fn conversion_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        goal: ConversionGoal,
        context: ExpressionContext,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ConversionTarget>, TyperError> {
        if let Some(instance) = goal.instance {
            let mut history = SearchHistory::default();
            if let Some(value) = self.choose_contextual_refs(
                refs.clone(),
                instance,
                context,
                tree_index,
                &mut history,
                journal,
            )? {
                return Ok(Some(ConversionTarget::Instance(
                    history.dictionary.emit(self, value, tree_index)?,
                )));
            }
        }
        let mut candidates = Vec::new();
        for reference in refs {
            let symbol = self.store.symbols.get(reference.symbol);
            if symbol.kind != SymbolKind::Method || !symbol.flags.contains(SymbolFlags::IMPLICIT) {
                continue;
            }
            let signature =
                self.contextual_reference_signature(reference, context.owner, tree_index, journal)?;
            if let Some(candidate) =
                self.legacy_conversion_candidate(reference, signature, goal, tree_index, journal)?
            {
                candidates.push((candidate, None));
            }
        }
        let Some(winner) =
            self.choose_contextual_candidate(candidates, goal.expected, tree_index, journal)?
        else {
            return Ok(None);
        };
        let function = self.provider_reference_tree(&winner, context, tree_index, journal)?;
        Ok(Some(ConversionTarget::Method {
            function,
            signature: winner.callable,
        }))
    }

    fn legacy_conversion_candidate(
        &mut self,
        reference: ContextualRef,
        signature: TypeId,
        goal: ConversionGoal,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<Provider>, TyperError> {
        let (method_type, arity) = match self.store.types.get(signature) {
            Type::Poly(poly) => (poly.result, poly.params.len()),
            _ => (signature, 0),
        };
        let Some(Type::Method(method)) = self.store.types.try_get(method_type) else {
            return Ok(None);
        };
        let [parameter] = method.params.as_slice() else {
            return Ok(None);
        };
        let input = parameter.ty;
        let output = method.result;
        if method.kind != MethodKind::Plain
            || parameter.erased
            || parameter.varargs
            || matches!(
                self.store.types.get(output),
                Type::Poly(_) | Type::Method(_)
            )
            || self.type_contains_param_ref(output, method_type)?
        {
            return Ok(None);
        }
        let mut callable = method_type;
        let mut type_arguments = Vec::new();
        if arity > 0 {
            let mut inferred = vec![None; arity];
            // Match the receiver at the declared input class, e.g. Child extends Box[Int].
            let views = if let Type::Applied { tycon, .. } = self.store.types.get(input)
                && let Some(class) = self.store.types.get(*tycon).reference_symbol()
                && self.type_contains_param_ref(input, signature)?
                && matches!(
                    self.store.types.get(goal.actual),
                    Type::TypeRef { .. } | Type::Applied { .. } | Type::ThisType { .. }
                ) {
                self.complete_relation_type(goal.actual, journal, &mut HashSet::new(), 0)?;
                self.nominal_views_as(goal.actual, class).map_err(|error| {
                    TyperError::ExpectedExpressionConformanceUnsupported {
                        source: self.source,
                        tree_index,
                        actual: goal.actual,
                        expected: input,
                        error: Box::new(error),
                    }
                })?
            } else {
                vec![goal.actual]
            };
            if views.is_empty() {
                return Ok(None);
            }
            let location = InferenceLocation {
                result_pattern: false,
                tree_index,
                argument_index: 0,
                depth: 0,
            };
            for view in views {
                match self.infer_type_constraints(
                    input,
                    view,
                    signature,
                    &mut inferred,
                    location,
                    journal,
                ) {
                    Ok(()) => {}
                    Err(
                        TyperError::UnsupportedInferenceShape { .. }
                        | TyperError::ConflictingInferenceConstraints { .. }
                        | TyperError::ApplicationArgumentTypeMismatch { .. },
                    ) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
            if inferred.contains(&None) {
                // The expected type can be wider in a covariant position. Keep
                // arguments fixed by the receiver and check result conformance below.
                let mut from_result = vec![None; arity];
                if !self.infer_provider_result(
                    signature,
                    output,
                    goal.expected,
                    &mut from_result,
                    tree_index,
                    journal,
                )? {
                    return Ok(None);
                }
                for (slot, result) in inferred.iter_mut().zip(from_result) {
                    *slot = slot.or(result);
                }
            }
            let Some(arguments) = inferred.into_iter().collect::<Option<Vec<_>>>() else {
                return Ok(None);
            };
            let Some(instance) =
                self.instantiate_provider_signature(signature, &arguments, tree_index, journal)?
            else {
                return Ok(None);
            };
            callable = instance;
            type_arguments = arguments;
        }
        let Some(Type::Method(method)) = self.store.types.try_get(callable) else {
            return Ok(None);
        };
        let input = method.params[0].ty;
        let output = method.result;
        if !self.contextual_conforms(goal.actual, input, tree_index, journal)?
            || !self.contextual_conforms(output, goal.expected, tree_index, journal)?
        {
            return Ok(None);
        }
        // Legacy conversion specificity compares the accepted inputs.
        Ok(Some(Provider {
            reference,
            type_arguments,
            callable,
            result: input,
        }))
    }
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct MemberConversionGoal {
    pub(in crate::typer) class: SymbolId,
    pub(in crate::typer) receiver: TypeId,
    pub(in crate::typer) name: Name,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn member_conversion_target(
        &mut self,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        for level in self.contextual_scopes(context, site)? {
            let refs = self.contextual_scope_refs(level, site, journal)?;
            if let Some(winner) =
                self.member_conversion_from_refs(refs, goal, context, site, journal)?
            {
                return Ok(Some(winner));
            }
        }
        let Some(refs) = self.adaptation_companion_refs(goal.receiver, site, journal)? else {
            return Ok(None);
        };
        self.member_conversion_from_refs(refs, goal, context, site, journal)
    }

    fn member_conversion_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TreeId<Typed>, TypeId)>, TyperError> {
        let mut candidates = Vec::new();
        for reference in refs {
            if self.initializing_local_symbols.contains(&reference.symbol) {
                continue;
            }
            let view =
                self.contextual_reference_signature(reference, context.owner, site, journal)?;
            let Some((base, input, output)) =
                self.member_conversion_signature(view, goal.class, site, journal)?
            else {
                continue;
            };
            if !self.contextual_conforms(goal.receiver, input, site, journal)? {
                continue;
            }
            // An abstract result can only offer members visible through its upper bound.
            let lookup_type = match self.store.types.get(output) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(param),
                    ..
                } if self.store.symbols.get(*param).kind == SymbolKind::TypeParameter => {
                    let info = self.complete_signature_parameter(*param, journal)?;
                    match self.store.types.get(info) {
                        Type::Bounds { high, .. } => *high,
                        _ => output,
                    }
                }
                _ => output,
            };
            let members = self
                .lookup_members_journaled(lookup_type, goal.name, journal)
                .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
            for member in members {
                if self.is_selected_member_accessible(
                    member.symbol,
                    member.declaring_class,
                    context,
                    site,
                )? {
                    candidates.push((
                        Provider {
                            reference,
                            type_arguments: Vec::new(),
                            callable: base,
                            result: view,
                        },
                        None,
                    ));
                    break;
                }
            }
        }
        let winner = self
            .choose_contextual_candidate(candidates, goal.receiver, site, journal)
            .map_err(|err| match err {
                TyperError::AmbiguousContextualArgument { candidates, .. } => {
                    TyperError::AmbiguousMemberConversion {
                        source: self.source,
                        tree_index: site,
                        receiver: goal.receiver,
                        name: goal.name,
                        candidates,
                    }
                }
                other => other,
            })?;
        let Some(winner) = winner else {
            return Ok(None);
        };
        let Some(Type::Applied { args, .. }) = self.store.types.try_get(winner.callable) else {
            return Err(TyperError::MemberTypeUnavailable {
                symbol: winner.reference.symbol,
            });
        };
        let output = args[1];
        let evidence =
            self.contextual_reference_tree(winner.reference, context, site, None, journal)?;
        Ok(Some((evidence, output)))
    }

    /// Project a monomorphic evidence value to its canonical `Conversion[In, Out]` base.
    fn member_conversion_signature(
        &mut self,
        view: TypeId,
        target: SymbolId,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TypeId, TypeId, TypeId)>, TyperError> {
        let head = match self.store.types.get(view) {
            Type::Applied { tycon, .. } => *tycon,
            _ => view,
        };
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) = self.store.types.try_get(head)
        else {
            return Ok(None);
        };
        let entry = self.store.symbols.get(*class);
        if !matches!(
            entry.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) || matches!(entry.origin, SymbolOrigin::Builtin)
        {
            return Ok(None);
        }
        self.complete_relation_type(view, journal, &mut HashSet::new(), 0)?;
        let bases = self.nominal_views_as(view, target).map_err(|error| {
            TyperError::ExpectedExpressionConformanceUnsupported {
                source: self.source,
                tree_index: site,
                actual: view,
                expected: {
                    let prefix = self.type_symbol_prefix(target);
                    self.store.types.alloc(Type::type_ref(prefix, target))
                },
                error: Box::new(error),
            }
        })?;
        let Some((&base, rest)) = bases.split_first() else {
            return Ok(None);
        };
        for &other in rest {
            if !self.contextual_conforms(base, other, site, journal)?
                || !self.contextual_conforms(other, base, site, journal)?
            {
                return Err(TyperError::UsingApplicationDeferred {
                    source: self.source,
                    tree_index: site,
                });
            }
        }
        let Some(Type::Applied { args, .. }) = self.store.types.try_get(base) else {
            return Ok(None);
        };
        let [input, output] = args[..] else {
            return Ok(None);
        };
        Ok(Some((base, input, output)))
    }
}
