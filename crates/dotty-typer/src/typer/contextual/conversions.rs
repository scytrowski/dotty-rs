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
            if let Some((candidate, _)) = self.legacy_conversion_candidate(
                reference,
                signature,
                goal.actual,
                Some(goal.expected),
                tree_index,
                journal,
            )? {
                candidates.push((candidate, Ok(())));
            }
        }
        let Some((winner, ())) =
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

    fn infer_conversion_input(
        &mut self,
        signature: TypeId,
        input: TypeId,
        actual: TypeId,
        inferred: &mut [Option<TypeId>],
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<bool, TyperError> {
        // Match the receiver at the declared input class, e.g. Child extends Box[Int].
        let views = if let Type::Applied { tycon, .. } = self.store.types.get(input)
            && let Some(class) = self.store.types.get(*tycon).reference_symbol()
            && self.type_contains_param_ref(input, signature)?
            && matches!(
                self.store.types.get(actual),
                Type::TypeRef { .. } | Type::Applied { .. } | Type::ThisType { .. }
            ) {
            self.complete_relation_type(actual, journal, &mut HashSet::new(), 0)?;
            self.nominal_views_as(actual, class).map_err(|error| {
                TyperError::ExpectedExpressionConformanceUnsupported {
                    source: self.source,
                    tree_index,
                    actual,
                    expected: input,
                    error: Box::new(error),
                }
            })?
        } else {
            vec![actual]
        };
        if views.is_empty() {
            return Ok(false);
        }
        let location = InferenceLocation {
            result_pattern: false,
            tree_index,
            argument_index: 0,
            depth: 0,
        };
        for view in views {
            match self.infer_type_constraints(input, view, signature, inferred, location, journal) {
                Ok(()) => {}
                Err(
                    TyperError::UnsupportedInferenceShape { .. }
                    | TyperError::ConflictingInferenceConstraints { .. }
                    | TyperError::ApplicationArgumentTypeMismatch { .. },
                ) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }

    fn legacy_conversion_candidate(
        &mut self,
        reference: ContextualRef,
        signature: TypeId,
        actual: TypeId,
        expected: Option<TypeId>,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(Provider, TypeId)>, TyperError> {
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
            if !self.infer_conversion_input(
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
                && let Some(expected) = expected
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
        let output = method.result;
        if !self.contextual_conforms(actual, input, tree_index, journal)? {
            return Ok(None);
        }
        if let Some(expected) = expected
            && !self.contextual_conforms(output, expected, tree_index, journal)?
        {
            return Ok(None);
        }
        // Legacy conversion specificity compares the accepted inputs.
        Ok(Some((
            Provider {
                reference,
                type_arguments,
                callable,
                result: input,
            },
            output,
        )))
    }
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct MemberConversionGoal {
    pub(in crate::typer) class: Option<SymbolId>,
    pub(in crate::typer) receiver: TypeId,
    pub(in crate::typer) name: Name,
}

#[derive(Default)]
struct MemberConversionSearch {
    history: SearchHistory,
    dependency_failure: Option<TyperError>,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn member_conversion_target(
        &mut self,
        goal: MemberConversionGoal,
        context: ExpressionContext,
        site: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(ConversionTarget, TypeId)>, TyperError> {
        let mut search = MemberConversionSearch::default();
        for level in self.contextual_scopes(context, site)? {
            let refs = self.contextual_scope_refs(level, site, journal)?;
            if let Some(winner) =
                self.member_conversion_from_refs(refs, goal, context, site, &mut search, journal)?
            {
                return Ok(Some(winner));
            }
        }
        if let Some(refs) = self.adaptation_companion_refs(goal.receiver, site, journal)?
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
        search: &mut MemberConversionSearch,
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
                    goal.receiver,
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
                let Some(method) = self.legacy_conversion_candidate(
                    reference,
                    view,
                    goal.receiver,
                    None,
                    site,
                    journal,
                )?
                else {
                    continue;
                };
                method
            };
            if !self.conversion_result_has_member(output, goal.name, context, site, journal)? {
                continue;
            }
            if matches!(self.store.types.get(candidate.callable), Type::Method(m) if m.kind != MethodKind::Plain)
            {
                match self.try_contextual_provider(
                    &candidate,
                    candidate.result,
                    context,
                    site,
                    &mut search.history,
                    None,
                ) {
                    Ok(_) => {}
                    Err(
                        cause @ (TyperError::MissingContextualArgument { .. }
                        | TyperError::AmbiguousContextualArgument { .. }
                        | TyperError::DivergentContextualArgument { .. }),
                    ) => {
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
                    Err(error) => return Err(error),
                }
            }
            candidates.push((candidate, Ok(output)));
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
        let Some((winner, output)) = winner else {
            return Ok(None);
        };
        let target = if matches!(self.store.types.get(winner.callable), Type::Method(method) if method.kind == MethodKind::Plain)
        {
            ConversionTarget::Method {
                function: self.provider_reference_tree(&winner, context, site, journal)?,
                signature: winner.callable,
            }
        } else {
            let tree = self.materialize_provider(
                &winner,
                winner.result,
                context,
                site,
                &mut search.history,
                journal,
            )?;
            ConversionTarget::Instance(
                std::mem::take(&mut search.history.dictionary).emit(self, tree, site)?,
            )
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
        let (body, arity) = match self.store.types.get(signature) {
            Type::Poly(poly) => (poly.result, poly.params.len()),
            _ => (signature, 0),
        };
        let Some(result) = self.contextual_factory_result(body, site)? else {
            return Ok(None);
        };
        let Some((input, output)) =
            self.member_conversion_signature(result, class, site, journal)?
        else {
            return Ok(None);
        };
        let (type_arguments, callable, result, input, output) = if arity > 0 {
            let mut inferred = vec![None; arity];
            if !self.infer_conversion_input(
                signature,
                input,
                receiver,
                &mut inferred,
                site,
                journal,
            )? {
                return Ok(None);
            }
            let Some(arguments) = inferred.into_iter().collect::<Option<Vec<_>>>() else {
                return Ok(None);
            };
            let Some(callable) =
                self.instantiate_provider_signature(signature, &arguments, site, journal)?
            else {
                return Ok(None);
            };
            let Some(result) = self.contextual_factory_result(callable, site)? else {
                return Ok(None);
            };
            let Some((input, output)) =
                self.member_conversion_signature(result, class, site, journal)?
            else {
                return Ok(None);
            };
            (arguments, callable, result, input, output)
        } else {
            (Vec::new(), signature, result, input, output)
        };
        if !self.contextual_conforms(receiver, input, site, journal)? {
            return Ok(None);
        }
        Ok(Some((
            Provider {
                reference,
                type_arguments,
                callable,
                result,
            },
            output,
        )))
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
