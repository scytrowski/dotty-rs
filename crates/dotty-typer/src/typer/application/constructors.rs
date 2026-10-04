//! Constructor call-site resolution and generic constructor inference.

use super::super::*;

/// One constructor declared directly by an instantiated class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstructorCandidate {
    /// Exact semantic identity of this constructor overload.
    pub symbol: SymbolId,
    /// Completed callable constructor signature.
    pub callable: TypeId,
    /// Class that directly declares the constructor.
    pub owner: SymbolId,
}

#[derive(Default)]
struct ConstructorApplicationCandidates {
    applicable_signatures: Vec<ConstructorCandidate>,
    rejected_incomplete: Vec<(SymbolId, OverloadRejection)>,
}

struct ConstructorOverloadRequest<'a> {
    function_tree: TreeId<Untyped>,
    argument_trees: &'a [TreeId<Untyped>],
    context: ExpressionContext,
    tree_index: u32,
    class: SymbolId,
    application_kind: ApplyKind,
    instance_type: TypeId,
    type_arguments: &'a [TypeId],
    qualifier: Option<TreeId<Typed>>,
    candidates: &'a [ConstructorCandidate],
    rejected_incomplete: &'a [(SymbolId, OverloadRejection)],
}

#[derive(Clone, Copy)]
struct ConstructorPolyInferenceRequest<'a> {
    constructor: SymbolId,
    binder: TypeId,
    poly: &'a PolyType,
    arguments: &'a [TypedArgument],
    application_kind: ApplyKind,
    tree_index: u32,
}

type ConstructorApplicationFunction = (TreeId<Typed>, TypeId, Option<Vec<TypedArgument>>);

impl SourceTyper<'_> {
    /// Finds constructors declared directly by the class represented by `instance_type`.
    /// This deliberately does not perform overload selection or inherit constructors.
    pub fn constructors_of(
        &mut self,
        instance_type: TypeId,
    ) -> Result<Vec<ConstructorCandidate>, TyperError> {
        self.run_atomic(|typer, info_journal| {
            typer.constructors_of_inner(instance_type, info_journal)
        })
    }

    pub(in crate::typer) fn constructors_of_inner(
        &mut self,
        instance_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<ConstructorCandidate>, TyperError> {
        let class = self.instantiable_class_of_type(instance_type, info_journal)?;
        self.constructor_candidates_for_class(class, info_journal)
    }

    pub(in crate::typer) fn constructor_candidates_for_class(
        &mut self,
        class: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<ConstructorCandidate>, TyperError> {
        let bucket = self.constructor_symbols_for_class(class, info_journal)?;
        let mut candidates = Vec::with_capacity(bucket.len());
        for symbol in bucket {
            let callable = match *self.store.symbols.info(symbol) {
                SymbolInfo::Complete(callable) => callable,
                SymbolInfo::Missing if self.index.definition_of(symbol).is_some() => {
                    self.complete_symbol_inner(symbol, info_journal)?
                }
                SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => {
                    return Err(TyperError::DeferredSymbolCompletion { symbol });
                }
            };
            if !self.is_constructor_callable(callable) {
                return Err(TyperError::MalformedConstructorCandidate { symbol, callable });
            }
            candidates.push(ConstructorCandidate {
                symbol,
                callable,
                owner: class,
            });
        }
        Ok(candidates)
    }

    fn constructor_application_candidates_for_class(
        &mut self,
        class: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<ConstructorApplicationCandidates, TyperError> {
        let bucket = self.constructor_symbols_for_class(class, info_journal)?;
        let mut result = ConstructorApplicationCandidates::default();
        for symbol in bucket {
            let callable = match *self.store.symbols.info(symbol) {
                SymbolInfo::Complete(callable) => Some(callable),
                SymbolInfo::Missing if self.index.definition_of(symbol).is_some() => {
                    match self.complete_symbol_inner(symbol, info_journal) {
                        Ok(callable) => Some(callable),
                        Err(TyperError::SecondaryConstructorTypeParametersUnsupported {
                            ..
                        }) => {
                            result
                                .rejected_incomplete
                                .push((symbol, OverloadRejection::IncompleteSignature));
                            None
                        }
                        Err(TyperError::DeferredSymbolCompletion { .. }) => {
                            result
                                .rejected_incomplete
                                .push((symbol, OverloadRejection::IncompleteSignature));
                            None
                        }
                        Err(error) => return Err(error),
                    }
                }
                SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => {
                    result
                        .rejected_incomplete
                        .push((symbol, OverloadRejection::IncompleteSignature));
                    None
                }
            };
            let Some(callable) = callable else {
                continue;
            };
            if !self.is_constructor_callable(callable) {
                return Err(TyperError::MalformedConstructorCandidate { symbol, callable });
            }
            result.applicable_signatures.push(ConstructorCandidate {
                symbol,
                callable,
                owner: class,
            });
        }
        Ok(result)
    }

    pub(in crate::typer) fn constructor_symbols_for_class(
        &mut self,
        class: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<SymbolId>, TyperError> {
        let class_info = self.ensure_constructor_class_info(class, info_journal)?;
        let declarations = match self.store.types.try_get(class_info) {
            Some(Type::ClassInfo(info)) if info.class == class => info.declarations,
            _ => {
                return Err(TyperError::MalformedClassInfo {
                    symbol: class,
                    info: class_info,
                });
            }
        };
        if !self.store.scopes.contains(declarations) {
            return Err(TyperError::NewClassInfoUnavailable { symbol: class });
        }
        let constructor_name = dotty_core::Name::new(
            self.store.names.intern("<init>"),
            dotty_core::Namespace::Term,
        );
        let bucket = self
            .store
            .scopes
            .get(declarations)
            .lookup_all(&constructor_name)
            .to_vec();
        for &symbol in &bucket {
            if !self.store.symbols.contains(symbol)
                || self.store.symbols.get(symbol).kind != SymbolKind::Constructor
                || self.store.symbols.get(symbol).owner != Some(class)
            {
                return Err(TyperError::MalformedConstructorBucket { class, symbol });
            }
        }
        Ok(bucket)
    }

    pub(in crate::typer) fn is_constructor_selection(&self, tree: TreeId<Untyped>) -> bool {
        let Some(Tree {
            kind: TreeKind::Select(selection),
            ..
        }) = self.arena.try_get(tree)
        else {
            return false;
        };
        !selection.backquoted
            && self.store.names.resolve(selection.name.text()) == "<init>"
            && self
                .arena
                .try_get(selection.qualifier)
                .is_some_and(|qualifier| matches!(qualifier.kind, TreeKind::New(_)))
    }

    pub(in crate::typer) fn resolve_constructor_application_function(
        &mut self,
        request: ApplicationRequest<'_>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<ConstructorApplicationFunction>, TyperError> {
        let ApplicationRequest {
            function_tree,
            argument_trees,
            application_kind,
            context,
            tree_index: application_tree_index,
        } = request;
        if !self.is_constructor_selection(function_tree) {
            return Ok(None);
        }
        let Some(function_node) = self.arena.try_get(function_tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: function_tree.index(),
            });
        };
        let TreeKind::Select(selection) = function_node.kind else {
            return Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: function_tree.index(),
                expression_kind: tree_kind_name(&function_node.kind),
            });
        };
        let qualifier_node = self.arena.try_get(selection.qualifier).cloned();
        let raw_info = if qualifier_node
            .as_ref()
            .is_some_and(|node| matches!(node.kind, TreeKind::New(_)))
        {
            self.raw_generic_new_class(selection.qualifier, context, info_journal)?
        } else {
            None
        };
        let raw_new = raw_info.is_some();
        let (qualifier, mut instance_type, class, instance_class, type_arguments) =
            if let Some((class, raw_type)) = raw_info {
                (None, raw_type, class, class, Vec::new())
            } else {
                let qualifier = self.type_expression_inner(
                    selection.qualifier,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let instance_type = self.typed_arena.get(qualifier).ty;
                let class = self.instantiable_class_of_type(instance_type, info_journal)?;
                let (instance_class, type_arguments) =
                    self.constructor_instance_type_arguments(instance_type, info_journal)?;
                (
                    Some(qualifier),
                    instance_type,
                    class,
                    instance_class,
                    type_arguments,
                )
            };
        let candidate_set =
            self.constructor_application_candidates_for_class(class, info_journal)?;
        let candidates = candidate_set.applicable_signatures;
        let candidate = match candidates.as_slice() {
            [] if candidate_set.rejected_incomplete.is_empty() => {
                return Err(TyperError::ConstructorApplicationUnavailable { class });
            }
            [] => {
                return Err(TyperError::ConstructorApplicationNoApplicable {
                    source: self.source,
                    tree_index: application_tree_index,
                    class,
                    candidates: candidate_set.rejected_incomplete,
                });
            }
            [candidate] if candidate_set.rejected_incomplete.is_empty() => candidate,
            _ => {
                return self.resolve_constructor_overload_application(
                    ConstructorOverloadRequest {
                        function_tree,
                        argument_trees,
                        context,
                        tree_index: application_tree_index,
                        class,
                        application_kind,
                        instance_type,
                        type_arguments: &type_arguments,
                        qualifier,
                        candidates: &candidates,
                        rejected_incomplete: &candidate_set.rejected_incomplete,
                    },
                    info_journal,
                    new_mappings,
                );
            }
        };
        if candidate.owner != instance_class || instance_class != class {
            return Err(TyperError::ConstructorInstanceClassMismatch {
                constructor: candidate.symbol,
                owner: candidate.owner,
                instance_class,
            });
        }
        let candidate_callable = self.store.types.try_get(candidate.callable).cloned();
        let (callable, instantiated_generic, inferred_arguments) = match candidate_callable {
            Some(Type::Poly(poly)) if raw_new => {
                let mut arguments = Vec::with_capacity(argument_trees.len());
                for argument_tree in argument_trees {
                    let typed = self.type_value_expression_inner(
                        *argument_tree,
                        context,
                        info_journal,
                        new_mappings,
                    )?;
                    let own_type = self.typed_arena.get(typed).ty;
                    let widened_type =
                        self.widen_expression_type_journaled(own_type, info_journal, 0)?;
                    arguments.push(TypedArgument {
                        typed,
                        own_type,
                        widened_type,
                    });
                }
                let type_arguments = self.infer_constructor_poly_arguments(
                    ConstructorPolyInferenceRequest {
                        constructor: candidate.symbol,
                        binder: candidate.callable,
                        poly: &poly,
                        arguments: &arguments,
                        application_kind,
                        tree_index: application_tree_index,
                    },
                    info_journal,
                )?;
                let instantiated = dotty_core::types::instantiate_poly(
                    self.store,
                    candidate.callable,
                    &type_arguments,
                )
                .map_err(|error| {
                    TyperError::ConstructorPolyInstantiationFailed {
                        constructor: candidate.symbol,
                        error,
                    }
                })?;
                for (parameter_index, (argument, bounds)) in type_arguments
                    .iter()
                    .copied()
                    .zip(instantiated.bounds.iter().copied())
                    .enumerate()
                {
                    self.check_constructor_type_argument_bounds(
                        candidate.symbol,
                        argument,
                        bounds,
                        parameter_index,
                        application_tree_index,
                        info_journal,
                    )?;
                }
                let result = self.validate_constructor_application_callable(
                    candidate.symbol,
                    instantiated.result,
                    application_tree_index,
                )?;
                let inferred_instance_type = self.constructor_result_instance_type(
                    result,
                    candidate.symbol,
                    application_tree_index,
                    info_journal,
                )?;
                instance_type = inferred_instance_type;
                self.finalize_raw_generic_new(
                    selection.qualifier,
                    inferred_instance_type,
                    candidate.symbol,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                (instantiated.result, true, Some(arguments))
            }
            Some(Type::Poly(poly)) if !poly.params.is_empty() || !type_arguments.is_empty() => {
                if type_arguments.len() != poly.params.len() {
                    return Err(TyperError::ConstructorTypeArgumentArityMismatch {
                        source: self.source,
                        tree_index: application_tree_index,
                        constructor: candidate.symbol,
                        expected: poly.params.len(),
                        actual: type_arguments.len(),
                    });
                }
                let instantiated = dotty_core::types::instantiate_poly(
                    self.store,
                    candidate.callable,
                    &type_arguments,
                )
                .map_err(|error| {
                    TyperError::ConstructorPolyInstantiationFailed {
                        constructor: candidate.symbol,
                        error,
                    }
                })?;
                for (parameter_index, (argument, bounds)) in type_arguments
                    .iter()
                    .copied()
                    .zip(instantiated.bounds.iter().copied())
                    .enumerate()
                {
                    self.check_constructor_type_argument_bounds(
                        candidate.symbol,
                        argument,
                        bounds,
                        parameter_index,
                        application_tree_index,
                        info_journal,
                    )?;
                }
                (instantiated.result, true, None)
            }
            Some(Type::Poly(_)) => (candidate.callable, false, None),
            Some(_) if !type_arguments.is_empty() => {
                return Err(TyperError::ConstructorCallableNotPolymorphic {
                    source: self.source,
                    tree_index: application_tree_index,
                    constructor: candidate.symbol,
                    instance_type,
                });
            }
            Some(_) => (candidate.callable, false, None),
            None => {
                return Err(TyperError::MalformedConstructorCandidate {
                    symbol: candidate.symbol,
                    callable: candidate.callable,
                });
            }
        };
        let constructor_result = self.validate_constructor_application_callable(
            candidate.symbol,
            callable,
            application_tree_index,
        )?;
        if instantiated_generic && !raw_new {
            self.require_constructor_result_matches_instance(
                candidate.symbol,
                constructor_result,
                instance_type,
                application_tree_index,
                info_journal,
            )?;
        }
        let qualifier = match qualifier {
            Some(qualifier) => qualifier,
            None => self
                .typed_index
                .get(self.source, selection.qualifier)
                .ok_or(TyperError::UnableToFinalizeRawGenericNewInstanceType {
                    source: self.source,
                    tree_index: application_tree_index,
                    constructor: candidate.symbol,
                    result: instance_type,
                })?,
        };
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: instance_type,
            target: TermRefTarget::Symbol(candidate.symbol),
        });
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
            qualifier,
            selection.name,
            selection.backquoted,
            ty,
            function_node.position,
        );
        self.typed_index
            .insert(self.source, function_tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, function_tree));
        Ok(Some((typed, callable, inferred_arguments)))
    }

    pub(in crate::typer) fn constructor_instance_type_arguments(
        &mut self,
        instance_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(SymbolId, Vec<TypeId>), TyperError> {
        let normalized = self.normalize_constructor_target(instance_type, info_journal)?;
        let (tycon, arguments) = match self.store.types.try_get(normalized) {
            Some(Type::Applied { tycon, args }) => (*tycon, args.clone()),
            _ => (normalized, Vec::new()),
        };
        let target = self.normalize_constructor_target(tycon, info_journal)?;
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(class),
            ..
        }) = self.store.types.try_get(target)
        else {
            return Err(TyperError::NewTargetNotClass { ty: instance_type });
        };
        Ok((*class, arguments))
    }

    pub(in crate::typer) fn constructor_overload_callable(
        &mut self,
        candidate: ConstructorCandidate,
        instance_type: TypeId,
        type_arguments: &[TypeId],
        has_qualifier: bool,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if !has_qualifier || type_arguments.is_empty() {
            return Ok(candidate.callable);
        }
        let adapt_owner = || MemberCandidate {
            symbol: candidate.symbol,
            declaring_class: candidate.owner,
            receiver_view: instance_type,
            inheritance_depth: 0,
        };
        let Some(Type::Poly(poly)) = self.store.types.try_get(candidate.callable).cloned() else {
            return self.member_type_on_journaled(&adapt_owner(), info_journal);
        };
        let is_primary = self
            .owner_primary_constructor_tree(candidate.symbol, candidate.owner)?
            .is_some_and(|tree| self.index.symbol_at(self.source, tree) == Some(candidate.symbol));
        if !is_primary {
            return self.member_type_on_journaled(&adapt_owner(), info_journal);
        }
        if type_arguments.len() != poly.params.len() {
            return Err(TyperError::ConstructorTypeArgumentArityMismatch {
                source: self.source,
                tree_index,
                constructor: candidate.symbol,
                expected: poly.params.len(),
                actual: type_arguments.len(),
            });
        }
        let instantiated =
            dotty_core::types::instantiate_poly(self.store, candidate.callable, type_arguments)
                .map_err(|error| TyperError::ConstructorPolyInstantiationFailed {
                    constructor: candidate.symbol,
                    error,
                })?;
        for (parameter_index, (argument, bounds)) in type_arguments
            .iter()
            .copied()
            .zip(instantiated.bounds.iter().copied())
            .enumerate()
        {
            self.check_constructor_type_argument_bounds(
                candidate.symbol,
                argument,
                bounds,
                parameter_index,
                tree_index,
                info_journal,
            )?;
        }
        Ok(instantiated.result)
    }

    fn resolve_constructor_overload_application(
        &mut self,
        request: ConstructorOverloadRequest<'_>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<ConstructorApplicationFunction>, TyperError> {
        let ConstructorOverloadRequest {
            function_tree,
            argument_trees,
            context,
            tree_index,
            class,
            application_kind,
            instance_type,
            type_arguments,
            qualifier,
            candidates,
            rejected_incomplete,
        } = request;
        let mut arguments = Vec::with_capacity(argument_trees.len());
        for argument_tree in argument_trees {
            let typed = self.type_value_expression_inner(
                *argument_tree,
                context,
                info_journal,
                new_mappings,
            )?;
            let own_type = self.typed_arena.get(typed).ty;
            let widened_type = self.widen_expression_type_journaled(own_type, info_journal, 0)?;
            arguments.push(TypedArgument {
                typed,
                own_type,
                widened_type,
            });
        }

        let mut completed_types = HashSet::new();
        for argument in &arguments {
            self.complete_relation_type(
                argument.widened_type,
                info_journal,
                &mut completed_types,
                0,
            )?;
        }
        let candidate_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let journal_checkpoint = info_journal.len();
        let mut application_candidates = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let callable = self.constructor_overload_callable(
                *candidate,
                instance_type,
                type_arguments,
                qualifier.is_some(),
                tree_index,
                info_journal,
            )?;
            let rejection = None;
            match self.store.types.try_get(callable).cloned() {
                Some(Type::Method(method)) => {
                    for parameter in &method.params {
                        self.complete_relation_type(
                            parameter.ty,
                            info_journal,
                            &mut completed_types,
                            0,
                        )?;
                    }
                }
                Some(Type::Poly(poly)) => {
                    let Some(Type::Method(method)) = self.store.types.try_get(poly.result).cloned()
                    else {
                        return Err(TyperError::MalformedConstructorCandidate {
                            symbol: candidate.symbol,
                            callable: candidate.callable,
                        });
                    };
                    for parameter in &method.params {
                        self.complete_relation_type(
                            parameter.ty,
                            info_journal,
                            &mut completed_types,
                            0,
                        )?;
                    }
                }
                _ => {
                    return Err(TyperError::MalformedConstructorCandidate {
                        symbol: candidate.symbol,
                        callable: candidate.callable,
                    });
                }
            }
            application_candidates.push(ApplicationCandidate {
                symbol: candidate.symbol,
                callable,
                member: None,
                rejection,
            });
        }

        let winner = self
            .choose_constructor_overload_candidate(
                &mut application_candidates,
                &arguments,
                tree_index,
                application_kind,
                info_journal,
            )
            .map_err(|error| match error {
                TyperError::OverloadApplicationNoApplicable { candidates, .. } => {
                    let mut candidates = candidates;
                    candidates.extend_from_slice(rejected_incomplete);
                    TyperError::ConstructorApplicationNoApplicable {
                        source: self.source,
                        tree_index,
                        class,
                        candidates,
                    }
                }
                TyperError::AmbiguousOverloadApplication { candidates, .. } => {
                    TyperError::AmbiguousConstructorApplication {
                        source: self.source,
                        tree_index,
                        class,
                        candidates,
                    }
                }
                TyperError::OverloadResolutionRequiresUnsupportedCandidate {
                    candidate, ..
                } => TyperError::ConstructorOverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    class,
                    candidate,
                },
                other => other,
            })?;
        let winner_symbol = winner.symbol;
        for (symbol, previous) in info_journal[journal_checkpoint..].iter().rev().copied() {
            if self.store.symbols.contains(symbol) {
                self.store.symbols.set_info(symbol, previous);
            }
        }
        info_journal.truncate(journal_checkpoint);
        self.resolver.rollback_to(self.store, resolver_checkpoint);
        self.store.rollback_to(candidate_checkpoint);
        self.type_index.restore(type_index_checkpoint);
        completed_types.clear();
        if let Some((candidate, _)) = rejected_incomplete.first() {
            return Err(
                TyperError::ConstructorOverloadResolutionRequiresUnsupportedCandidate {
                    source: self.source,
                    tree_index,
                    class,
                    candidate: *candidate,
                },
            );
        }
        let selected_candidate = candidates
            .iter()
            .find(|candidate| candidate.symbol == winner_symbol)
            .copied()
            .ok_or(TyperError::MalformedConstructorCandidate {
                symbol: winner_symbol,
                callable: winner.callable,
            })?;
        if qualifier.is_none()
            && type_arguments.is_empty()
            && self
                .constructor_target_class_arity(class)?
                .is_some_and(|arity| arity > 0)
            && self
                .owner_primary_constructor_tree(selected_candidate.symbol, class)?
                .is_none_or(|tree| {
                    self.index.symbol_at(self.source, tree) != Some(selected_candidate.symbol)
                })
        {
            return Err(
                TyperError::UnsupportedRawGenericSecondaryConstructorInference {
                    source: self.source,
                    tree_index,
                    constructor: selected_candidate.symbol,
                    owner: class,
                },
            );
        }
        let selected_callable = self.constructor_overload_callable(
            selected_candidate,
            instance_type,
            type_arguments,
            qualifier.is_some(),
            tree_index,
            info_journal,
        )?;
        if let Some(Type::Method(method)) = self.store.types.try_get(selected_callable).cloned() {
            for parameter in &method.params {
                self.complete_relation_type(parameter.ty, info_journal, &mut completed_types, 0)?;
            }
        }
        let mut selected_application = [ApplicationCandidate {
            symbol: winner_symbol,
            callable: selected_callable,
            member: None,
            rejection: None,
        }];
        let winner = self.choose_constructor_overload_candidate(
            &mut selected_application,
            &arguments,
            tree_index,
            application_kind,
            info_journal,
        )?;
        let result = self.validate_constructor_application_callable(
            winner.symbol,
            winner.callable,
            tree_index,
        )?;
        let qualifier = match qualifier {
            Some(qualifier) => {
                self.require_constructor_result_matches_instance(
                    winner.symbol,
                    result,
                    instance_type,
                    tree_index,
                    info_journal,
                )?;
                qualifier
            }
            None => {
                let instance_type = self.constructor_result_instance_type(
                    result,
                    winner.symbol,
                    tree_index,
                    info_journal,
                )?;
                let Some(TreeKind::Select(selection)) =
                    self.arena.try_get(function_tree).map(|tree| &tree.kind)
                else {
                    return Err(TyperError::MalformedConstructorCandidate {
                        symbol: winner.symbol,
                        callable: winner.callable,
                    });
                };
                self.finalize_raw_generic_new(
                    selection.qualifier,
                    instance_type,
                    winner.symbol,
                    context,
                    info_journal,
                    new_mappings,
                )?
            }
        };
        let instance_type = self.typed_arena.get(qualifier).ty;

        let Some(function_node) = self.arena.try_get(function_tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: function_tree.index(),
            });
        };
        let TreeKind::Select(selection) = function_node.kind else {
            return Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: function_tree.index(),
                expression_kind: tree_kind_name(&function_node.kind),
            });
        };
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: instance_type,
            target: TermRefTarget::Symbol(winner.symbol),
        });
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
            qualifier,
            selection.name,
            selection.backquoted,
            ty,
            function_node.position,
        );
        self.typed_index
            .insert(self.source, function_tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, function_tree));
        Ok(Some((typed, winner.callable, Some(arguments))))
    }

    pub(in crate::typer) fn raw_generic_new_class(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<(SymbolId, TypeId)>, TyperError> {
        if self.typed_index.get(self.source, tree).is_some() {
            return Ok(None);
        }
        let Some(TreeKind::New(new)) = self.arena.try_get(tree).map(|node| &node.kind) else {
            return Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index: tree.index(),
                expression_kind: "not a New expression",
            });
        };
        let type_context = self.expression_type_context(context)?;
        let raw_type = self.type_of_tpt_inner_journaled(new.tpt, type_context, info_journal)?;
        let (class, arguments) =
            self.constructor_instance_type_arguments(raw_type, info_journal)?;
        if !arguments.is_empty() {
            return Ok(None);
        }
        match self.constructor_target_class_arity(class)? {
            Some(arity) if arity > 0 => Ok(Some((class, raw_type))),
            _ => Ok(None),
        }
    }

    pub(in crate::typer) fn prepare_raw_generic_constructor_chain(
        &mut self,
        root: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<(), TyperError> {
        let mut reversed_clauses = Vec::new();
        let mut current = root;
        loop {
            let Some(node) = self.arena.try_get(current) else {
                return Ok(());
            };
            let TreeKind::Apply(application) = &node.kind else {
                break;
            };
            reversed_clauses.push((application.kind, application.args.clone()));
            current = application.function;
        }
        let Some(node) = self.arena.try_get(current) else {
            return Ok(());
        };
        let TreeKind::Select(selection) = &node.kind else {
            return Ok(());
        };
        if self.store.names.resolve(selection.name.text()) != "<init>" || selection.backquoted {
            return Ok(());
        }
        let Some((class, _raw_type)) =
            self.raw_generic_new_class(selection.qualifier, context, info_journal)?
        else {
            return Ok(());
        };
        reversed_clauses.reverse();
        let candidates = self.constructor_candidates_for_class(class, info_journal)?;
        let candidate = match candidates.as_slice() {
            [candidate] => *candidate,
            [] => return Err(TyperError::ConstructorApplicationUnavailable { class }),
            _ if candidates.iter().any(|candidate| {
                matches!(
                    self.store.types.try_get(candidate.callable),
                    Some(Type::Poly(_))
                )
            }) =>
            {
                return Ok(());
            }
            _ => {
                return Err(TyperError::ConstructorOverloadResolutionDeferred {
                    source: self.source,
                    tree_index: root.index(),
                    class,
                    candidates: candidates
                        .iter()
                        .map(|candidate| candidate.symbol)
                        .collect(),
                });
            }
        };
        let Some(Type::Poly(poly)) = self.store.types.try_get(candidate.callable).cloned() else {
            return Err(TyperError::UnableToFinalizeRawGenericNewInstanceType {
                source: self.source,
                tree_index: root.index(),
                constructor: candidate.symbol,
                result: candidate.callable,
            });
        };
        let mut method_type = poly.result;
        let mut typed_clauses = Vec::with_capacity(reversed_clauses.len());
        for (application_kind, clause) in reversed_clauses {
            let Some(Type::Method(method)) = self.store.types.try_get(method_type).cloned() else {
                return Err(TyperError::ApplicationCalleeNotMethod {
                    source: self.source,
                    tree_index: root.index(),
                    ty: method_type,
                });
            };
            if !application_kind_accepts(application_kind, method.kind) {
                return Err(TyperError::ApplicationMethodKindMismatch {
                    source: self.source,
                    tree_index: root.index(),
                    application_kind,
                    method_kind: method.kind,
                });
            }
            if method.params.len() != clause.len() {
                return Err(TyperError::ApplicationArityMismatch {
                    source: self.source,
                    tree_index: root.index(),
                    expected: method.params.len(),
                    actual: clause.len(),
                });
            }
            let mut typed_arguments = Vec::with_capacity(clause.len());
            for argument_tree in clause {
                let typed = self.type_value_expression_inner(
                    argument_tree,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let own_type = self.typed_arena.get(typed).ty;
                let widened_type =
                    self.widen_expression_type_journaled(own_type, info_journal, 0)?;
                if self.typed_index.get(self.source, argument_tree).is_none() {
                    self.typed_index
                        .insert(self.source, argument_tree, typed)
                        .map_err(|error| TyperError::ConflictingTypedExpression {
                            source: error.source,
                            tree_index: error.untyped.index(),
                            existing: error.existing.index(),
                            attempted: error.attempted.index(),
                        })?;
                    new_mappings.push((self.source, argument_tree));
                }
                typed_arguments.push(TypedArgument {
                    typed,
                    own_type,
                    widened_type,
                });
            }
            method_type = method.result;
            typed_clauses.push((method, typed_arguments));
        }
        let type_arguments = self.infer_constructor_poly_clauses(
            candidate.symbol,
            candidate.callable,
            &poly,
            &typed_clauses,
            root.index(),
            info_journal,
        )?;
        let instantiated =
            dotty_core::types::instantiate_poly(self.store, candidate.callable, &type_arguments)
                .map_err(|error| TyperError::ConstructorPolyInstantiationFailed {
                    constructor: candidate.symbol,
                    error,
                })?;
        for (parameter_index, (argument, bounds)) in type_arguments
            .iter()
            .copied()
            .zip(instantiated.bounds.iter().copied())
            .enumerate()
        {
            self.check_constructor_type_argument_bounds(
                candidate.symbol,
                argument,
                bounds,
                parameter_index,
                root.index(),
                info_journal,
            )?;
        }
        let result = self.validate_constructor_application_callable(
            candidate.symbol,
            instantiated.result,
            root.index(),
        )?;
        let instance_type = self.constructor_result_instance_type(
            result,
            candidate.symbol,
            root.index(),
            info_journal,
        )?;
        self.finalize_raw_generic_new(
            selection.qualifier,
            instance_type,
            candidate.symbol,
            context,
            info_journal,
            new_mappings,
        )?;
        Ok(())
    }

    pub(in crate::typer) fn finalize_raw_generic_new(
        &mut self,
        tree: TreeId<Untyped>,
        instance_type: TypeId,
        constructor: SymbolId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let (class, _) = self
            .raw_generic_new_class(tree, context, info_journal)?
            .ok_or(TyperError::UnableToFinalizeRawGenericNewInstanceType {
                source: self.source,
                tree_index: tree.index(),
                constructor,
                result: instance_type,
            })?;
        let inferred_class = self.instantiable_class_of_type(instance_type, info_journal)?;
        if inferred_class != class {
            return Err(TyperError::UnableToFinalizeRawGenericNewInstanceType {
                source: self.source,
                tree_index: tree.index(),
                constructor,
                result: instance_type,
            });
        }
        self.ensure_constructor_class_info(class, info_journal)?;
        let Some(source_tree) = self.arena.try_get(tree).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        let TreeKind::New(new) = source_tree.kind else {
            return Err(TyperError::UnableToFinalizeRawGenericNewInstanceType {
                source: self.source,
                tree_index: tree.index(),
                constructor: class,
                result: instance_type,
            });
        };
        let typed_tpt = self.reify_constructor_type_tree(new.tpt, instance_type, new_mappings)?;
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).new_expr(
            typed_tpt,
            instance_type,
            source_tree.position,
        );
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(typed)
    }

    pub(in crate::typer) fn constructor_result_instance_type(
        &mut self,
        result: TypeId,
        constructor: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        let (class, arguments) = self
            .constructor_instance_type_arguments(result, info_journal)
            .map_err(|_| TyperError::UnableToFinalizeRawGenericNewInstanceType {
                source: self.source,
                tree_index,
                constructor,
                result,
            })?;
        let owner = self.store.symbols.get(constructor).owner.ok_or(
            TyperError::MalformedConstructorCandidate {
                symbol: constructor,
                callable: result,
            },
        )?;
        if class != owner || arguments.is_empty() {
            return Err(TyperError::UnableToFinalizeRawGenericNewInstanceType {
                source: self.source,
                tree_index,
                constructor,
                result,
            });
        }
        Ok(result)
    }

    fn infer_constructor_poly_arguments(
        &mut self,
        request: ConstructorPolyInferenceRequest<'_>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<TypeId>, TyperError> {
        let ConstructorPolyInferenceRequest {
            constructor,
            binder,
            poly,
            arguments,
            application_kind,
            tree_index,
        } = request;
        self.infer_poly_application_arguments(
            binder,
            poly,
            arguments,
            application_kind,
            tree_index,
            info_journal,
        )
        .map_err(|error| match error {
            TyperError::UnconstrainedTypeParameter {
                parameter_index, ..
            } => TyperError::UnconstrainedConstructorTypeParameter {
                source: self.source,
                tree_index,
                constructor,
                parameter_index,
            },
            TyperError::ConflictingInferenceConstraints {
                parameter_index,
                first,
                second,
                ..
            } => TyperError::ConflictingConstructorInferenceConstraints {
                source: self.source,
                tree_index,
                constructor,
                parameter_index,
                first,
                second,
            },
            TyperError::UnsupportedInferenceShape {
                parameter_index,
                formal,
                actual,
                ..
            } => TyperError::UnsupportedConstructorInferenceShape {
                source: self.source,
                tree_index,
                constructor,
                parameter_index,
                formal,
                actual,
            },
            TyperError::UnsupportedPolymorphicApplicationShape { binder, .. } => {
                TyperError::UnsupportedConstructorInferenceShape {
                    source: self.source,
                    tree_index,
                    constructor,
                    parameter_index: None,
                    formal: binder,
                    actual: binder,
                }
            }
            other => other,
        })
    }

    pub(in crate::typer) fn infer_constructor_poly_clauses(
        &mut self,
        constructor: SymbolId,
        binder: TypeId,
        poly: &PolyType,
        clauses: &[(MethodType, Vec<TypedArgument>)],
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<TypeId>, TyperError> {
        let mut inferred = vec![None; poly.params.len()];
        for (method, arguments) in clauses {
            if method.params.len() != arguments.len() {
                return Err(TyperError::ApplicationArityMismatch {
                    source: self.source,
                    tree_index,
                    expected: method.params.len(),
                    actual: arguments.len(),
                });
            }
            for (argument_index, (parameter, argument)) in
                method.params.iter().zip(arguments).enumerate()
            {
                if parameter.erased
                    || parameter.varargs
                    || matches!(
                        self.store.types.try_get(parameter.ty),
                        Some(Type::ByName { .. })
                    )
                {
                    return Err(TyperError::UnsupportedConstructorInferenceShape {
                        source: self.source,
                        tree_index,
                        constructor,
                        parameter_index: None,
                        formal: parameter.ty,
                        actual: argument.widened_type,
                    });
                }
                self.infer_type_constraints(
                    parameter.ty,
                    argument.widened_type,
                    binder,
                    &mut inferred,
                    InferenceLocation {
                        tree_index,
                        argument_index,
                        depth: 0,
                    },
                    info_journal,
                )
                .map_err(|error| match error {
                    TyperError::UnconstrainedTypeParameter {
                        parameter_index, ..
                    } => TyperError::UnconstrainedConstructorTypeParameter {
                        source: self.source,
                        tree_index,
                        constructor,
                        parameter_index,
                    },
                    TyperError::ConflictingInferenceConstraints {
                        parameter_index,
                        first,
                        second,
                        ..
                    } => TyperError::ConflictingConstructorInferenceConstraints {
                        source: self.source,
                        tree_index,
                        constructor,
                        parameter_index,
                        first,
                        second,
                    },
                    TyperError::UnsupportedInferenceShape {
                        parameter_index,
                        formal,
                        actual,
                        ..
                    } => TyperError::UnsupportedConstructorInferenceShape {
                        source: self.source,
                        tree_index,
                        constructor,
                        parameter_index,
                        formal,
                        actual,
                    },
                    other => other,
                })?;
            }
        }
        inferred
            .into_iter()
            .enumerate()
            .map(|(parameter_index, ty)| {
                ty.ok_or(TyperError::UnconstrainedConstructorTypeParameter {
                    source: self.source,
                    tree_index,
                    constructor,
                    parameter_index,
                })
            })
            .collect()
    }

    pub(in crate::typer) fn check_constructor_type_argument_bounds(
        &mut self,
        constructor: SymbolId,
        argument: TypeId,
        bounds: TypeId,
        parameter_index: usize,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        self.check_explicit_type_argument_bounds(
            argument,
            bounds,
            parameter_index,
            tree_index,
            info_journal,
        )
        .map_err(|error| match error {
            TyperError::ExplicitTypeArgumentBoundViolation {
                source,
                tree_index,
                parameter_index,
                argument,
                bound,
                side,
            } => TyperError::ConstructorTypeArgumentBoundViolation {
                source,
                tree_index,
                constructor,
                parameter_index,
                argument,
                bound,
                side,
            },
            TyperError::UnsupportedExplicitTypeArgumentBounds {
                source,
                tree_index,
                parameter_index,
                bounds,
            } => TyperError::UnsupportedConstructorTypeArgumentBounds {
                source,
                tree_index,
                constructor,
                parameter_index,
                bounds,
            },
            TyperError::ExplicitTypeArgumentBoundCheckUnsupported {
                source,
                tree_index,
                parameter_index,
                argument,
                bound,
                error,
            } => TyperError::ConstructorTypeArgumentBoundCheckUnsupported {
                source,
                tree_index,
                constructor,
                parameter_index,
                argument,
                bound,
                error,
            },
            other => other,
        })
    }

    pub(in crate::typer) fn require_constructor_result_matches_instance(
        &mut self,
        constructor: SymbolId,
        constructor_result: TypeId,
        instance_type: TypeId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let (result_class, result_arguments) =
            self.constructor_instance_type_arguments(constructor_result, info_journal)?;
        let (instance_class, instance_arguments) =
            self.constructor_instance_type_arguments(instance_type, info_journal)?;
        if result_class != instance_class || result_arguments.len() != instance_arguments.len() {
            return Err(TyperError::ConstructorResultTypeMismatch {
                source: self.source,
                tree_index,
                constructor,
                constructor_result,
                instance_type,
            });
        }
        for (result_argument, instance_argument) in
            result_arguments.into_iter().zip(instance_arguments)
        {
            let mut seen = HashSet::new();
            self.complete_relation_type(result_argument, info_journal, &mut seen, 0)?;
            self.complete_relation_type(instance_argument, info_journal, &mut seen, 0)?;
            match (
                self.conforms(result_argument, instance_argument),
                self.conforms(instance_argument, result_argument),
            ) {
                (Ok(true), Ok(true)) => {}
                (Ok(_), Ok(_)) => {
                    return Err(TyperError::ConstructorResultTypeMismatch {
                        source: self.source,
                        tree_index,
                        constructor,
                        constructor_result,
                        instance_type,
                    });
                }
                (Err(error), _) | (_, Err(error)) => {
                    return Err(TyperError::ConstructorResultTypeCheckUnsupported {
                        source: self.source,
                        tree_index,
                        constructor,
                        constructor_result,
                        instance_type,
                        error: Box::new(error),
                    });
                }
            }
        }
        Ok(())
    }

    pub(in crate::typer) fn validate_constructor_application_callable(
        &self,
        symbol: SymbolId,
        callable: TypeId,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        let mut current = callable;
        let mut visited = HashSet::new();
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            if !visited.insert(current) {
                return Err(TyperError::MalformedConstructorCandidate { symbol, callable });
            }
            match self.store.types.try_get(current) {
                Some(Type::Poly(_)) => {
                    return Err(TyperError::ConstructorPolymorphicApplicationDeferred {
                        source: self.source,
                        tree_index,
                    });
                }
                Some(Type::Method(method)) => {
                    current = method.result;
                }
                Some(_) => return Ok(current),
                None => {
                    return Err(TyperError::MalformedConstructorCandidate { symbol, callable });
                }
            }
        }
        Err(TyperError::MalformedConstructorCandidate { symbol, callable })
    }

    pub(in crate::typer) fn instantiable_class_of_type(
        &mut self,
        instance_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<SymbolId, TyperError> {
        let normalized = self.normalize_constructor_target(instance_type, info_journal)?;
        let (tycon, actual_arity) = match self.store.types.try_get(normalized) {
            Some(Type::Applied { tycon, args }) => (*tycon, args.len()),
            _ => (normalized, 0),
        };
        let target = self.normalize_constructor_target(tycon, info_journal)?;
        let symbol = match self.store.types.try_get(target) {
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            }) => *symbol,
            _ => {
                return Err(TyperError::NewTargetNotClass { ty: instance_type });
            }
        };
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        let semantic = self.store.symbols.get(symbol);
        match semantic.kind {
            SymbolKind::Class => {
                match self.constructor_target_class_arity(symbol)? {
                    Some(expected_arity) if actual_arity != expected_arity => {
                        return Err(TyperError::ConstructorTargetGenericArityMismatch {
                            class: symbol,
                            expected: expected_arity,
                            actual: actual_arity,
                        });
                    }
                    None if actual_arity > 0 => {
                        return Err(TyperError::ExternalGenericInstantiationDeferred {
                            class: symbol,
                        });
                    }
                    Some(_) | None => {}
                }
                if semantic.flags.contains(SymbolFlags::ABSTRACT) {
                    return Err(TyperError::AbstractClassInstantiation { symbol });
                }
                Ok(symbol)
            }
            SymbolKind::Trait => Err(TyperError::TraitInstantiation { symbol }),
            SymbolKind::ModuleClass | SymbolKind::Object => {
                Err(TyperError::ModuleInstantiation { symbol })
            }
            SymbolKind::Package => Err(TyperError::PackageInstantiation { symbol }),
            SymbolKind::TypeParameter => Err(TyperError::TypeParameterInstantiation { symbol }),
            _ => Err(TyperError::NewTargetNotClass { ty: instance_type }),
        }
    }

    pub(in crate::typer) fn constructor_target_class_arity(
        &self,
        class: SymbolId,
    ) -> Result<Option<usize>, TyperError> {
        let Some(definition) = self.index.definition_of(class) else {
            return Ok(None);
        };
        let SourceDefinition::Canonical { source, tree } = definition else {
            return Err(TyperError::SourceClassTypeParametersProvenanceMissing { symbol: class });
        };
        if source != self.source {
            return Err(TyperError::SourceClassTypeParametersProvenanceMissing { symbol: class });
        }
        let Some(class_node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: tree.index(),
            });
        };
        let TreeKind::TypeDef(class_definition) = &class_node.kind else {
            return Err(TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: tree.index(),
            });
        };
        let Some(template_node) = self.arena.try_get(class_definition.rhs) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: class_definition.rhs.index(),
            });
        };
        let TreeKind::Template(template) = &template_node.kind else {
            return Err(TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: class_definition.rhs.index(),
            });
        };
        let Some(constructor_node) = self.arena.try_get(template.constructor) else {
            return Err(TyperError::TreeOutsideArena {
                source,
                tree_index: template.constructor.index(),
            });
        };
        let TreeKind::DefDef(constructor) = &constructor_node.kind else {
            return Err(TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: template.constructor.index(),
            });
        };
        let constructor_symbol = self.index.symbol_at(source, template.constructor).ok_or(
            TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: template.constructor.index(),
            },
        )?;
        for parameter_tree in &constructor.type_params {
            let Some(parameter_node) = self.arena.try_get(*parameter_tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source,
                    tree_index: parameter_tree.index(),
                });
            };
            if !matches!(parameter_node.kind, TreeKind::TypeDef(_)) {
                return Err(TyperError::MalformedSourceClassTypeParameters {
                    class,
                    tree_index: parameter_tree.index(),
                });
            }
            let parameter = self
                .index
                .derived_symbol_at(constructor_symbol, source, *parameter_tree)
                .ok_or(TyperError::ClassTypeParameterSymbolMissing {
                    class,
                    tree_index: parameter_tree.index(),
                })?;
            if !self.store.symbols.contains(parameter)
                || self.store.symbols.get(parameter).kind != SymbolKind::TypeParameter
            {
                return Err(TyperError::MalformedSourceClassTypeParameters {
                    class,
                    tree_index: parameter_tree.index(),
                });
            }
        }
        Ok(Some(constructor.type_params.len()))
    }

    pub(in crate::typer) fn normalize_constructor_target(
        &mut self,
        ty: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            match crate::types::TypeNormalizer::new(self.store).normalize_for_lookup(ty) {
                Ok(normalized) => return Ok(normalized),
                Err(crate::types::TypeNormalizeError::AliasInfoIncomplete { symbol, .. })
                    if self.index.definition_of(symbol).is_some() =>
                {
                    self.complete_symbol_inner(symbol, info_journal)?;
                }
                Err(error) => {
                    return Err(TyperError::ConstructorLookupNormalization { ty, error });
                }
            }
        }
        Err(TyperError::ConstructorLookupNormalization {
            ty,
            error: crate::types::TypeNormalizeError::TooDeep,
        })
    }

    pub(in crate::typer) fn is_constructor_callable(&self, callable: TypeId) -> bool {
        let mut current = callable;
        let mut visited = HashSet::new();
        for _ in 0..crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            if !visited.insert(current) {
                return false;
            }
            match self.store.types.try_get(current) {
                Some(Type::Method(_)) => return true,
                Some(Type::Poly(poly)) => current = poly.result,
                _ => return false,
            }
        }
        false
    }

    pub(in crate::typer) fn ensure_constructor_class_info(
        &mut self,
        class: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if !self.store.symbols.contains(class) {
            return Err(TyperError::UnknownSymbol { symbol: class });
        }
        let info = match *self.store.symbols.info(class) {
            SymbolInfo::Complete(info) => info,
            SymbolInfo::Missing if self.index.definition_of(class).is_some() => {
                self.complete_symbol_inner(class, info_journal)?
            }
            SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => {
                return Err(TyperError::NewClassInfoUnavailable { symbol: class });
            }
        };
        self.validate_existing_class_info(class, info)?;
        let Type::ClassInfo(class_info) =
            self.store
                .types
                .try_get(info)
                .ok_or(TyperError::MalformedClassInfo {
                    symbol: class,
                    info,
                })?
        else {
            return Err(TyperError::MalformedClassInfo {
                symbol: class,
                info,
            });
        };
        if !self.store.scopes.contains(class_info.declarations) {
            return Err(TyperError::NewClassInfoUnavailable { symbol: class });
        }
        Ok(info)
    }
}
