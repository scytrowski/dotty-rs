//! Selects either a Conversion instance or an old-style unary implicit method.

use super::*;

pub(in crate::typer) enum ConversionTarget {
    Instance(TreeId<Typed>),
    Applied(TreeId<Typed>),
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct ConversionOperand {
    pub(in crate::typer) source: TreeId<Untyped>,
    pub(in crate::typer) tree: TreeId<Typed>,
    pub(in crate::typer) ty: TypeId,
}

#[derive(Clone, Copy)]
pub(in crate::typer) enum ConversionUse {
    Expected(TypeId),
    Member { name: Name, site: u32 },
}

// A nested ambiguity participates in ranking until a strictly better conversion wins.
type ConversionCandidate = (Provider, Result<TypeId, TyperError>);

#[derive(Clone, Copy)]
pub(in crate::typer) struct ConversionGoal {
    pub(in crate::typer) instance: Option<TypeId>,
    pub(in crate::typer) operand: ConversionOperand,
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
            None => &[goal.operand.ty, goal.expected],
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
            let mut instance_refs = Vec::with_capacity(refs.len());
            for &reference in &refs {
                if !self.is_unresolved_conversion_feature_import(reference)? {
                    instance_refs.push(reference);
                }
            }
            let mut history = SearchHistory::default();
            if let Some(value) = self.choose_contextual_refs(
                instance_refs,
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
            if let Some(candidate) = self.legacy_conversion_candidate(
                reference,
                signature,
                goal.operand,
                ConversionUse::Expected(goal.expected),
                context,
                journal,
            )? {
                candidates.push(candidate);
            }
        }
        let Some((winner, _)) =
            self.choose_contextual_candidate(candidates, goal.expected, tree_index, journal)?
        else {
            return Ok(None);
        };
        Ok(Some(ConversionTarget::Applied(
            self.apply_legacy_conversion(&winner, goal.operand, context, journal)?,
        )))
    }

    // Scala 3.9 declares this library val as languageFeature.implicitConversions,
    // a marker trait unrelated to Conversion. Its nested type can be unresolved
    // on a TASTy-only classpath. Keep ordinary contextual searches independent.
    fn is_unresolved_conversion_feature_import(
        &self,
        reference: ContextualRef,
    ) -> Result<bool, TyperError> {
        let field = self.store.symbols.get(reference.symbol);
        if field.kind != SymbolKind::Field
            || !matches!(field.origin, SymbolOrigin::Tasty(_))
            || self.store.names.resolve(field.name.text()) != "implicitConversions"
            || !matches!(field.info, SymbolInfo::Complete(ty) if matches!(self.store.types.get(ty), Type::Error(_)))
        {
            return Ok(false);
        }
        let Some(ContextualReceiver::Import(qualifier)) = reference.receiver else {
            return Ok(false);
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(object),
            ..
        } = self.store.types.get(self.typed_arena.get(qualifier).ty)
        else {
            return Ok(false);
        };
        let owner = self.store.symbols.get(*object);
        Ok(owner.kind == SymbolKind::Object
            && matches!(owner.origin, SymbolOrigin::Tasty(_))
            && self.store.names.resolve(owner.name.text()) == "language"
            && owner
                .owner
                .is_some_and(|package| self.package_path(package) == ["scala"])
            && field.owner == Some(self.module_class_of_object(*object)?))
    }

    fn legacy_conversion_candidate(
        &mut self,
        reference: ContextualRef,
        signature: TypeId,
        operand: ConversionOperand,
        purpose: ConversionUse,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<ConversionCandidate>, TyperError> {
        let tree_index = operand.source.index();
        let actual = operand.ty;
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
        if method.kind != MethodKind::Plain
            || parameter.erased
            || parameter.varargs
            || self.type_contains_param_ref(method.result, method_type)?
        {
            return Ok(None);
        }
        let Some(output) = self.legacy_conversion_output(method.result)? else {
            return Ok(None);
        };
        let mut callable = method_type;
        let mut type_arguments = Vec::new();
        if arity > 0 {
            let mut inferred = vec![None; arity];
            if !self.infer_contextual_receiver(
                signature,
                input,
                actual,
                &mut inferred,
                tree_index,
                journal,
            )? {
                return Ok(None);
            }
            if inferred.contains(&None)
                && let ConversionUse::Expected(expected) = purpose
            {
                // The expected type can be wider in a covariant position. Keep
                // arguments fixed by the receiver and check result conformance below.
                let mut from_result = vec![None; arity];
                if !self.infer_provider_result(
                    signature,
                    output,
                    expected,
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
        let tail = method.result;
        let Some(output) = self.legacy_conversion_output(tail)? else {
            return Ok(None);
        };
        if !self.contextual_conforms(actual, input, tree_index, journal)? {
            return Ok(None);
        }
        let applicable = match purpose {
            ConversionUse::Expected(expected) => {
                self.contextual_conforms(output, expected, tree_index, journal)?
            }
            ConversionUse::Member { name, site } => {
                self.conversion_result_has_member(output, name, context, site, journal)?
            }
        };
        if !applicable {
            return Ok(None);
        }
        // Legacy conversion specificity compares the accepted inputs.
        let provider = Provider {
            reference,
            type_arguments,
            callable,
            result: input,
        };
        let mut viability = Ok(output);
        if matches!(self.store.types.get(tail), Type::Method(_)) {
            // Probe the actual call so defaults can replay its receiver and arguments.
            match self.semantic_attempt(None, |typer, nested| {
                typer.apply_legacy_conversion(&provider, operand, context, nested)
            }) {
                Ok(_) => {}
                Err(
                    TyperError::MissingContextualArgument { .. }
                    | TyperError::DivergentContextualArgument { .. },
                ) => return Ok(None),
                Err(cause @ TyperError::AmbiguousContextualArgument { .. }) => {
                    viability = Err(match purpose {
                        ConversionUse::Expected(_) => cause,
                        ConversionUse::Member { name, .. } => {
                            TyperError::MemberConversionDependency {
                                source: self.source,
                                tree_index,
                                name,
                                provider: reference.symbol,
                                cause: Box::new(cause),
                            }
                        }
                    });
                }
                Err(error) => return Err(error),
            }
        }
        Ok(Some((provider, viability)))
    }

    fn legacy_conversion_output(&self, tail: TypeId) -> Result<Option<TypeId>, TyperError> {
        let output = match self.store.types.get(tail) {
            Type::Method(method)
                if method.kind == MethodKind::Implicit
                    && !self.type_contains_param_ref(method.result, tail)? =>
            {
                method.result
            }
            _ => tail,
        };
        Ok((!matches!(
            self.store.types.get(output),
            Type::Method(_) | Type::Poly(_)
        ))
        .then_some(output))
    }

    fn apply_legacy_conversion(
        &mut self,
        provider: &Provider,
        operand: ConversionOperand,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let function =
            self.provider_reference_tree(provider, context, operand.source.index(), journal)?;
        let Type::Method(method) = self.store.types.get(provider.callable) else {
            return Err(TyperError::MemberTypeUnavailable {
                symbol: provider.reference.symbol,
            });
        };
        let tail = method.result;
        let position = self.arena.get(operand.source).position;
        let call = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
            function,
            vec![operand.tree],
            tail,
            position,
        );
        if matches!(self.store.types.get(tail), Type::Method(_)) {
            self.apply_contextual_arguments(operand.source, call, context, journal)
        } else {
            Ok(call)
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct MemberConversionGoal {
    pub(in crate::typer) class: Option<SymbolId>,
    pub(in crate::typer) operand: ConversionOperand,
    pub(in crate::typer) name: Name,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn member_conversion_target(
        &mut self,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(ConversionTarget, TypeId)>, TyperError> {
        let mut search = AdaptationSearch::default();
        for level in self.contextual_scopes(context, site)? {
            let refs = self.contextual_scope_refs(level, site, journal)?;
            if let Some(winner) =
                self.member_conversion_from_refs(refs, goal, context, site, &mut search, journal)?
            {
                return Ok(Some(winner));
            }
        }
        if let Some(refs) = self.adaptation_companion_refs(goal.operand.ty, site, journal)?
            && let Some(winner) =
                self.member_conversion_from_refs(refs, goal, context, site, &mut search, journal)?
        {
            return Ok(Some(winner));
        }
        match search.dependency_failure {
            Some(error) => Err(error),
            None => Ok(None),
        }
    }

    fn member_conversion_from_refs(
        &mut self,
        refs: Vec<ContextualRef>,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        search: &mut AdaptationSearch,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(ConversionTarget, TypeId)>, TyperError> {
        let mut candidates = Vec::new();
        for reference in refs {
            if self.initializing_local_symbols.contains(&reference.symbol) {
                continue;
            }
            let view =
                self.contextual_reference_signature(reference, context.owner, site, journal)?;
            let (candidate, output) = if let Some(class) = goal.class
                && let Some(instance) = self.member_conversion_provider(
                    reference,
                    view,
                    class,
                    goal.operand.ty,
                    site,
                    journal,
                )? {
                instance
            } else {
                let entry = self.store.symbols.get(reference.symbol);
                if entry.kind != SymbolKind::Method || !entry.flags.contains(SymbolFlags::IMPLICIT)
                {
                    continue;
                }
                if let Some(candidate) = self.legacy_conversion_candidate(
                    reference,
                    view,
                    goal.operand,
                    ConversionUse::Member {
                        name: goal.name,
                        site,
                    },
                    context,
                    journal,
                )? {
                    candidates.push(candidate);
                }
                continue;
            };
            if !self.conversion_result_has_member(output, goal.name, context, site, journal)? {
                continue;
            }
            if let Some(cause) = search.dependency_error(self, &candidate, context, site)? {
                search.dependency_failure.get_or_insert_with(|| {
                    TyperError::MemberConversionDependency {
                        source: self.source,
                        tree_index: site,
                        name: goal.name,
                        provider: reference.symbol,
                        cause: Box::new(cause),
                    }
                });
                continue;
            }
            candidates.push((candidate, Ok(output)));
        }
        let winner = self
            .choose_contextual_candidate(candidates, goal.operand.ty, site, journal)
            .map_err(|err| match err {
                TyperError::AmbiguousContextualArgument { candidates, .. } => {
                    TyperError::AmbiguousMemberConversion {
                        source: self.source,
                        tree_index: site,
                        receiver: goal.operand.ty,
                        name: goal.name,
                        candidates,
                    }
                }
                other => other,
            })?;
        let Some((winner, output)) = winner else {
            return Ok(None);
        };
        let target = if matches!(self.store.types.get(winner.callable), Type::Method(method) if method.kind == MethodKind::Plain)
        {
            ConversionTarget::Applied(self.apply_legacy_conversion(
                &winner,
                goal.operand,
                context,
                journal,
            )?)
        } else {
            ConversionTarget::Instance(search.materialize(self, &winner, context, site, journal)?)
        };
        Ok(Some((target, output)))
    }

    fn conversion_result_has_member(
        &mut self,
        output: TypeId,
        name: Name,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
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
            .lookup_members_journaled(lookup_type, name, journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        for member in members {
            if self.is_selected_member_accessible(
                member.symbol,
                member.declaring_class,
                context,
                site,
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn member_conversion_provider(
        &mut self,
        reference: ContextualRef,
        signature: TypeId,
        class: SymbolId,
        receiver: TypeId,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(Provider, TypeId)>, TyperError> {
        let Some(open) = self.contextual_provider_head(reference, signature, site)? else {
            return Ok(None);
        };
        let Some((input, output)) =
            self.member_conversion_signature(open.result, class, site, journal)?
        else {
            return Ok(None);
        };
        let Some(provider) =
            self.instantiate_receiver_provider(open, input, receiver, site, journal)?
        else {
            return Ok(None);
        };
        let (input, output) = if provider.type_arguments.is_empty() {
            (input, output)
        } else {
            let Some(base) =
                self.member_conversion_signature(provider.result, class, site, journal)?
            else {
                return Ok(None);
            };
            base
        };
        if !self.contextual_conforms(receiver, input, site, journal)? {
            return Ok(None);
        }
        Ok(Some((provider, output)))
    }

    /// Extract the input and output of a canonical `Conversion[In, Out]` base.
    fn member_conversion_signature(
        &mut self,
        view: TypeId,
        target: SymbolId,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(TypeId, TypeId)>, TyperError> {
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
        Ok(Some((input, output)))
    }
}
