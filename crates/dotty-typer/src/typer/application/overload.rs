//! Candidate discovery, filtering, and selection for callable overloads.

use super::super::*;

#[derive(Clone, Copy)]
enum ApplicationFunctionShape {
    Ident(Ident),
    Select {
        selection: dotty_core::ast::Select<Untyped>,
        qualifier: TreeId<Typed>,
        receiver_type: TypeId,
    },
}

fn supported_override_signature(method: &MethodType, store: &SemanticStore) -> bool {
    method.kind == MethodKind::Plain
        && method.params.iter().all(|parameter| {
            !parameter.erased
                && !parameter.varargs
                && !matches!(store.types.try_get(parameter.ty), Some(Type::ByName { .. }))
        })
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn resolve_overloaded_application_function(
        &mut self,
        request: ApplicationRequest<'_>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<ResolvedApplicationFunction>, TyperError> {
        let ApplicationRequest {
            function_tree,
            argument_trees,
            application_kind,
            context,
            tree_index: application_tree_index,
        } = request;
        let Some(function_node) = self.arena.try_get(function_tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: function_tree.index(),
            });
        };
        let (mut candidates, function_shape) = match function_node.kind {
            TreeKind::Ident(ident) => {
                let candidates = self.expression_term_candidates(
                    ident.name,
                    context,
                    function_tree.index(),
                    function_node.position,
                )?;
                if candidates.len() <= 1 {
                    return Ok(None);
                }
                if candidates
                    .iter()
                    .any(|symbol| self.store.symbols.get(*symbol).kind != SymbolKind::Method)
                {
                    return Err(TyperError::MixedApplicationCandidateKinds {
                        source: self.source,
                        tree_index: application_tree_index,
                        candidates,
                    });
                }
                (
                    candidates
                        .into_iter()
                        .map(|symbol| {
                            let callable =
                                self.completed_expression_symbol_info(symbol, info_journal)?;
                            self.validate_overload_callable(symbol, callable)?;
                            Ok(ApplicationCandidate {
                                symbol,
                                callable,
                                member: None,
                                rejection: None,
                            })
                        })
                        .collect::<Result<Vec<_>, TyperError>>()?,
                    ApplicationFunctionShape::Ident(ident),
                )
            }
            TreeKind::Select(selection) if selection.name.is_term() => {
                let qualifier = self.type_expression_inner(
                    selection.qualifier,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let receiver_type = self.typed_arena.get(qualifier).ty;
                self.require_stable_selection_prefix(receiver_type, function_tree.index())?;
                let receiver =
                    self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
                let receiver_view = self.this_type_receiver_view(receiver)?;
                let members = self
                    .lookup_overload_members_journaled(receiver_view, selection.name, info_journal)
                    .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                if members.len() <= 1 {
                    return Ok(None);
                }
                if members
                    .iter()
                    .any(|member| self.store.symbols.get(member.symbol).kind != SymbolKind::Method)
                {
                    return Err(TyperError::MixedApplicationCandidateKinds {
                        source: self.source,
                        tree_index: application_tree_index,
                        candidates: members.iter().map(|member| member.symbol).collect(),
                    });
                }
                let candidates = members
                    .into_iter()
                    .map(|member| {
                        let callable = self.member_type_on_journaled(&member, info_journal)?;
                        self.validate_overload_callable(member.symbol, callable)?;
                        Ok(ApplicationCandidate {
                            symbol: member.symbol,
                            callable,
                            member: Some(member),
                            rejection: None,
                        })
                    })
                    .collect::<Result<Vec<_>, TyperError>>()?;
                (
                    candidates,
                    ApplicationFunctionShape::Select {
                        selection,
                        qualifier,
                        receiver_type,
                    },
                )
            }
            _ => return Ok(None),
        };

        let mut arguments = Vec::with_capacity(argument_trees.len());
        for argument_tree in argument_trees {
            let typed =
                self.type_expression_inner(*argument_tree, context, info_journal, new_mappings)?;
            let own_type = self.typed_arena.get(typed).ty;
            let widened_type = self.widen_expression_type_journaled(own_type, info_journal, 0)?;
            arguments.push(TypedArgument {
                typed,
                own_type,
                widened_type,
            });
        }

        let mut completed_relation_types = std::collections::HashSet::new();
        for argument in &arguments {
            self.complete_relation_type(
                argument.widened_type,
                info_journal,
                &mut completed_relation_types,
                0,
            )?;
        }
        for candidate in &candidates {
            if let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable) {
                let parameter_types: Vec<_> = method.params.iter().map(|param| param.ty).collect();
                for parameter_type in parameter_types {
                    self.complete_relation_type(
                        parameter_type,
                        info_journal,
                        &mut completed_relation_types,
                        0,
                    )?;
                }
            }
        }

        self.remove_overridden_overload_candidates(&mut candidates, application_tree_index)?;

        let winner = self.choose_method_overload_candidate(
            &mut candidates,
            &arguments,
            application_tree_index,
            application_kind,
            info_journal,
        )?;
        let function_type = match function_shape {
            ApplicationFunctionShape::Ident(ident) => {
                let ty = self.expression_type_of_symbol(
                    winner.symbol,
                    context.owner,
                    function_tree.index(),
                    info_journal,
                )?;
                TypedAstBuilder::new(&mut self.typed_arena).ident_with_backquoted(
                    ident.name,
                    ident.backquoted,
                    ty,
                    function_node.position,
                )
            }
            ApplicationFunctionShape::Select {
                selection,
                qualifier,
                receiver_type,
            } => {
                if winner
                    .member
                    .is_some_and(|member| member.symbol != winner.symbol)
                {
                    return Err(TyperError::MalformedOverloadCandidate {
                        symbol: winner.symbol,
                        callable: winner.callable,
                    });
                }
                let ty = self.store.types.alloc(Type::TermRef {
                    prefix: receiver_type,
                    target: TermRefTarget::Symbol(winner.symbol),
                });
                TypedAstBuilder::new(&mut self.typed_arena).select(
                    qualifier,
                    selection.name,
                    selection.backquoted,
                    ty,
                    function_node.position,
                )
            }
        };
        self.typed_index
            .insert(self.source, function_tree, function_type)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, function_tree));
        Ok(Some(ResolvedApplicationFunction {
            typed: function_type,
            callable: winner.callable,
            arguments,
        }))
    }

    pub(in crate::typer) fn choose_method_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ApplicationCandidate, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let journal_checkpoint = info_journal.len();
        let original_candidates = candidates.to_vec();

        let resolution = (|| {
            self.instantiate_generic_overload_candidates(
                candidates,
                arguments,
                tree_index,
                application_kind,
                info_journal,
            )?;
            self.choose_overload_candidate(candidates, arguments, tree_index, application_kind)
        })();
        let selected = resolution?;

        for (symbol, previous) in info_journal[journal_checkpoint..].iter().rev().copied() {
            if self.store.symbols.contains(symbol) {
                self.store.symbols.set_info(symbol, previous);
            }
        }
        info_journal.truncate(journal_checkpoint);
        self.store.rollback_to(store_checkpoint);
        self.type_index.restore(type_index_checkpoint);
        candidates.copy_from_slice(&original_candidates);

        let selected_index = candidates
            .iter()
            .position(|candidate| candidate.symbol == selected.symbol)
            .ok_or(TyperError::MalformedOverloadCandidate {
                symbol: selected.symbol,
                callable: selected.callable,
            })?;
        self.instantiate_generic_overload_candidates(
            &mut candidates[selected_index..=selected_index],
            arguments,
            tree_index,
            application_kind,
            info_journal,
        )?;
        Ok(candidates[selected_index])
    }

    pub(in crate::typer) fn choose_constructor_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ApplicationCandidate, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let journal_checkpoint = info_journal.len();
        let original_candidates = candidates.to_vec();

        let resolution = (|| {
            self.instantiate_generic_overload_candidates(
                candidates,
                arguments,
                tree_index,
                application_kind,
                info_journal,
            )?;
            self.choose_overload_candidate_with_policy(
                candidates,
                arguments,
                tree_index,
                application_kind,
                true,
            )
        })();
        let selected = resolution?;

        for (symbol, previous) in info_journal[journal_checkpoint..].iter().rev().copied() {
            if self.store.symbols.contains(symbol) {
                self.store.symbols.set_info(symbol, previous);
            }
        }
        info_journal.truncate(journal_checkpoint);
        self.store.rollback_to(store_checkpoint);
        self.type_index.restore(type_index_checkpoint);
        candidates.copy_from_slice(&original_candidates);

        let selected_index = candidates
            .iter()
            .position(|candidate| candidate.symbol == selected.symbol)
            .ok_or(TyperError::MalformedOverloadCandidate {
                symbol: selected.symbol,
                callable: selected.callable,
            })?;
        self.instantiate_generic_overload_candidates(
            &mut candidates[selected_index..=selected_index],
            arguments,
            tree_index,
            application_kind,
            info_journal,
        )?;
        Ok(candidates[selected_index])
    }

    pub(in crate::typer) fn instantiate_generic_overload_candidates(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        for candidate in candidates {
            let Some(Type::Poly(poly)) = self.store.types.try_get(candidate.callable).cloned()
            else {
                continue;
            };
            let Some(Type::Method(method)) = self.store.types.try_get(poly.result).cloned() else {
                continue;
            };
            if !application_kind_accepts(application_kind, method.kind) {
                candidate.rejection = Some(OverloadRejection::ApplicationKindMismatch {
                    application_kind,
                    method_kind: method.kind,
                });
                continue;
            }
            if overload_arity_rejection(&method, arguments.len()).is_some() {
                continue;
            }

            let type_arguments = match self.infer_poly_application_arguments(
                candidate.callable,
                &poly,
                arguments,
                application_kind,
                tree_index,
                info_journal,
            ) {
                Ok(type_arguments) => type_arguments,
                Err(TyperError::ApplicationArgumentTypeMismatch {
                    argument_index,
                    actual,
                    expected,
                    ..
                }) => {
                    candidate.rejection = Some(OverloadRejection::ArgumentNonConformance {
                        argument_index,
                        actual,
                        expected,
                    });
                    continue;
                }
                Err(TyperError::ConflictingInferenceConstraints {
                    parameter_index, ..
                }) => {
                    candidate.rejection = Some(OverloadRejection::TypeArgumentInferenceFailure {
                        parameter_index: Some(parameter_index),
                    });
                    continue;
                }
                Err(TyperError::ApplicationArityMismatch { .. }) => continue,
                Err(
                    TyperError::UnconstrainedTypeParameter { .. }
                    | TyperError::UnsupportedInferenceShape { .. }
                    | TyperError::UnsupportedPolymorphicApplicationShape { .. },
                ) => continue,
                Err(error) => return Err(error),
            };
            let instantiated = dotty_core::types::instantiate_poly(
                self.store,
                candidate.callable,
                &type_arguments,
            )
            .map_err(TyperError::PolyInstantiation)?;

            let mut supported_bounds = true;
            for (parameter_index, (argument, bounds)) in type_arguments
                .iter()
                .copied()
                .zip(instantiated.bounds.iter().copied())
                .enumerate()
            {
                match self.check_explicit_type_argument_bounds(
                    argument,
                    bounds,
                    parameter_index,
                    tree_index,
                    info_journal,
                ) {
                    Ok(()) => {}
                    Err(TyperError::ExplicitTypeArgumentBoundViolation { side, .. }) => {
                        candidate.rejection = Some(OverloadRejection::TypeArgumentBoundViolation {
                            parameter_index,
                            side,
                        });
                        supported_bounds = false;
                        break;
                    }
                    Err(
                        TyperError::UnsupportedExplicitTypeArgumentBounds { .. }
                        | TyperError::ExplicitTypeArgumentBoundCheckUnsupported { .. },
                    ) => {
                        supported_bounds = false;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if supported_bounds {
                candidate.callable = instantiated.result;
            }
        }
        Ok(())
    }

    pub(in crate::typer) fn validate_overload_callable(
        &self,
        symbol: SymbolId,
        callable: TypeId,
    ) -> Result<(), TyperError> {
        match self.store.types.try_get(callable) {
            Some(Type::Method(_) | Type::Poly(_)) => Ok(()),
            _ => Err(TyperError::MalformedOverloadCandidate { symbol, callable }),
        }
    }

    pub(in crate::typer) fn complete_relation_type(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        seen: &mut std::collections::HashSet<TypeId>,
        depth: usize,
    ) -> Result<(), TyperError> {
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::TypeNormalization(
                crate::types::TypeNormalizeError::TooDeep,
            ));
        }
        if !seen.insert(ty) {
            return Ok(());
        }
        let Some(node) = self.store.types.try_get(ty).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(ty) {
                    crate::types::TypeNormalizeError::UnfilledType { ty }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty }
                },
            ));
        };
        let mut children = Vec::new();
        match node {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                prefix,
            } => {
                if !self.store.symbols.contains(symbol) {
                    return Err(TyperError::UnknownSymbol { symbol });
                }
                children.push(prefix);
                if matches!(
                    self.store.symbols.get(symbol).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                ) && self.is_current_source_symbol(symbol)
                    && matches!(*self.store.symbols.info(symbol), SymbolInfo::Missing)
                {
                    self.complete_symbol_inner(symbol, info_journal)?;
                }
                if let SymbolInfo::Complete(info) = *self.store.symbols.info(symbol)
                    && let Some(Type::ClassInfo(class_info)) = self.store.types.try_get(info)
                {
                    children.extend(class_info.parents.iter().copied());
                }
            }
            Type::Applied { tycon, args } => {
                children.push(tycon);
                children.extend(args);
            }
            Type::TermRef { prefix, .. } => children.push(prefix),
            Type::SuperType {
                this_type,
                super_type,
            } => children.extend([this_type, super_type]),
            Type::Bounds { low, high } => children.extend([low, high]),
            Type::AliasingBounds { alias }
            | Type::ByName { result: alias }
            | Type::Flexible { underlying: alias }
            | Type::Recursive { parent: alias }
            | Type::Wildcard { bounds: alias }
            | Type::JavaArray { element: alias }
            | Type::Repeated { element: alias }
            | Type::Annotated {
                underlying: alias, ..
            } => children.push(alias),
            Type::And { left, right } | Type::Or { left, right } => {
                children.extend([left, right]);
            }
            _ => {}
        }
        for child in children {
            self.complete_relation_type(child, info_journal, seen, depth + 1)?;
        }
        Ok(())
    }

    pub(in crate::typer) fn remove_overridden_overload_candidates(
        &mut self,
        candidates: &mut Vec<ApplicationCandidate>,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let mut overridden = vec![false; candidates.len()];
        for derived_index in 0..candidates.len() {
            let Some(derived_member) = candidates[derived_index].member else {
                continue;
            };
            let Some(Type::Method(derived_method)) = self
                .store
                .types
                .try_get(candidates[derived_index].callable)
                .cloned()
            else {
                continue;
            };
            if !supported_override_signature(&derived_method, self.store)
                || self.type_contains_param_ref(
                    derived_method.result,
                    candidates[derived_index].callable,
                )?
            {
                continue;
            }

            for base_index in 0..candidates.len() {
                if base_index == derived_index || overridden[base_index] {
                    continue;
                }
                let Some(base_member) = candidates[base_index].member else {
                    continue;
                };
                if derived_member.declaring_class == base_member.declaring_class {
                    continue;
                }
                let Some(Type::Method(base_method)) = self
                    .store
                    .types
                    .try_get(candidates[base_index].callable)
                    .cloned()
                else {
                    continue;
                };
                if !supported_override_signature(&base_method, self.store)
                    || self.type_contains_param_ref(
                        base_method.result,
                        candidates[base_index].callable,
                    )?
                    || derived_method.params.len() != base_method.params.len()
                {
                    continue;
                }

                let mut same_parameters = true;
                for (derived, base) in derived_method.params.iter().zip(&base_method.params) {
                    let derived_subtype = self.is_subtype(derived.ty, base.ty).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[derived_index].symbol,
                        }
                    })?;
                    if !derived_subtype {
                        same_parameters = false;
                        break;
                    }
                    let base_subtype = self.is_subtype(base.ty, derived.ty).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[base_index].symbol,
                        }
                    })?;
                    if !base_subtype {
                        same_parameters = false;
                        break;
                    }
                }
                if !same_parameters {
                    continue;
                }

                let derived_view = derived_member.receiver_view;
                let base_view = base_member.receiver_view;
                let derived_is_subtype =
                    self.is_subtype(derived_view, base_view).map_err(|_| {
                        TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                            source: self.source,
                            tree_index,
                            candidate: candidates[derived_index].symbol,
                        }
                    })?;
                if !derived_is_subtype {
                    continue;
                }
                let base_is_subtype = self.is_subtype(base_view, derived_view).map_err(|_| {
                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                        source: self.source,
                        tree_index,
                        candidate: candidates[base_index].symbol,
                    }
                })?;
                if !base_is_subtype {
                    overridden[base_index] = true;
                }
            }
        }
        let mut index = 0;
        candidates.retain(|_| {
            let keep = !overridden[index];
            index += 1;
            keep
        });
        Ok(())
    }

    pub(in crate::typer) fn choose_overload_candidate(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
    ) -> Result<ApplicationCandidate, TyperError> {
        self.choose_overload_candidate_with_policy(
            candidates,
            arguments,
            tree_index,
            application_kind,
            false,
        )
    }

    pub(in crate::typer) fn choose_overload_candidate_with_policy(
        &mut self,
        candidates: &mut [ApplicationCandidate],
        arguments: &[TypedArgument],
        tree_index: u32,
        application_kind: ApplyKind,
        retain_unsupported_rejections: bool,
    ) -> Result<ApplicationCandidate, TyperError> {
        let mut applicable = Vec::new();
        let mut rejected = Vec::new();
        let mut unsupported_candidates = Vec::new();
        for candidate in candidates.iter().copied() {
            if let Some(rejection) = candidate.rejection {
                rejected.push((candidate.symbol, rejection));
                continue;
            }
            let method = match self.store.types.try_get(candidate.callable) {
                Some(Type::Method(method)) => method.clone(),
                Some(Type::Poly(poly)) => {
                    let rejection = match self.store.types.try_get(poly.result) {
                        Some(Type::Method(method))
                            if let Some(expected) =
                                overload_arity_rejection(method, arguments.len()) =>
                        {
                            OverloadRejection::WrongArity {
                                expected,
                                actual: arguments.len(),
                            }
                        }
                        _ => {
                            if retain_unsupported_rejections {
                                rejected.push((
                                    candidate.symbol,
                                    OverloadRejection::UnsupportedSemantics,
                                ));
                                unsupported_candidates.push(candidate.symbol);
                                continue;
                            }
                            return Err(
                                TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                    source: self.source,
                                    tree_index,
                                    candidate: candidate.symbol,
                                },
                            );
                        }
                    };
                    rejected.push((candidate.symbol, rejection));
                    continue;
                }
                _ => {
                    return Err(TyperError::MalformedOverloadCandidate {
                        symbol: candidate.symbol,
                        callable: candidate.callable,
                    });
                }
            };
            if !application_kind_accepts(application_kind, method.kind) {
                rejected.push((
                    candidate.symbol,
                    OverloadRejection::ApplicationKindMismatch {
                        application_kind,
                        method_kind: method.kind,
                    },
                ));
                continue;
            }
            if let Some(expected) = overload_arity_rejection(&method, arguments.len()) {
                rejected.push((
                    candidate.symbol,
                    OverloadRejection::WrongArity {
                        expected,
                        actual: arguments.len(),
                    },
                ));
                continue;
            }
            let mut argument_rejection = None;
            let mut unsupported_relation = false;
            for (argument_index, (argument, parameter)) in
                arguments.iter().zip(&method.params).enumerate()
            {
                match self.conforms(argument.widened_type, parameter.ty) {
                    Ok(true) => {}
                    Ok(false) => {
                        argument_rejection = Some(OverloadRejection::ArgumentNonConformance {
                            argument_index,
                            actual: argument.widened_type,
                            expected: parameter.ty,
                        });
                        break;
                    }
                    Err(_) => {
                        unsupported_relation = true;
                        break;
                    }
                }
            }
            if let Some(rejection) = argument_rejection {
                rejected.push((candidate.symbol, rejection));
                continue;
            }
            if unsupported_relation {
                if retain_unsupported_rejections {
                    rejected.push((candidate.symbol, OverloadRejection::UnsupportedSemantics));
                    unsupported_candidates.push(candidate.symbol);
                    continue;
                }
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: candidate.symbol,
                });
            }
            let unsupported = method.params.iter().any(|param| {
                param.erased
                    || param.varargs
                    || matches!(
                        self.store.types.try_get(param.ty),
                        Some(Type::ByName { .. })
                    )
            }) || self
                .type_contains_param_ref(method.result, candidate.callable)?;
            if unsupported {
                if retain_unsupported_rejections {
                    rejected.push((candidate.symbol, OverloadRejection::UnsupportedSemantics));
                    unsupported_candidates.push(candidate.symbol);
                    continue;
                }
                return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    candidate: candidate.symbol,
                });
            }
            applicable.push(candidate);
        }
        if !unsupported_candidates.is_empty()
            && (!applicable.is_empty() || !retain_unsupported_rejections)
        {
            return Err(TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                source: self.source,
                tree_index,
                candidate: unsupported_candidates[0],
            });
        }
        match applicable.as_slice() {
            [candidate] => Ok(*candidate),
            [] => Err(TyperError::OverloadApplicationNoApplicable {
                source: self.source,
                tree_index,
                candidates: rejected,
            }),
            many => {
                let mut most_specific = Vec::new();
                for candidate in many {
                    let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable)
                    else {
                        return Err(TyperError::MalformedOverloadCandidate {
                            symbol: candidate.symbol,
                            callable: candidate.callable,
                        });
                    };
                    let candidate_params: Vec<_> =
                        method.params.iter().map(|parameter| parameter.ty).collect();
                    let mut dominates_all = true;
                    for other in many {
                        if candidate.symbol == other.symbol {
                            continue;
                        }
                        let Some(Type::Method(other_method)) =
                            self.store.types.try_get(other.callable)
                        else {
                            return Err(TyperError::MalformedOverloadCandidate {
                                symbol: other.symbol,
                                callable: other.callable,
                            });
                        };
                        let other_params: Vec<_> = other_method
                            .params
                            .iter()
                            .map(|parameter| parameter.ty)
                            .collect();
                        let mut strictly_more_specific = false;
                        for (candidate_parameter, other_parameter) in
                            candidate_params.iter().zip(&other_params)
                        {
                            let candidate_is_subtype = self
                                .is_subtype(*candidate_parameter, *other_parameter)
                                .map_err(|_| {
                                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                        source: self.source,
                                        tree_index,
                                        candidate: candidate.symbol,
                                    }
                                })?;
                            if !candidate_is_subtype {
                                dominates_all = false;
                                break;
                            }
                            let other_is_subtype = self
                                .is_subtype(*other_parameter, *candidate_parameter)
                                .map_err(|_| {
                                    TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                                        source: self.source,
                                        tree_index,
                                        candidate: other.symbol,
                                    }
                                })?;
                            strictly_more_specific |= !other_is_subtype;
                        }
                        if !dominates_all || !strictly_more_specific {
                            dominates_all = false;
                            break;
                        }
                    }
                    if dominates_all {
                        most_specific.push(*candidate);
                    }
                }
                match most_specific.as_slice() {
                    [candidate] => Ok(*candidate),
                    _ => Err(TyperError::AmbiguousOverloadApplication {
                        source: self.source,
                        tree_index,
                        candidates: many.iter().map(|candidate| candidate.symbol).collect(),
                    }),
                }
            }
        }
    }
}
