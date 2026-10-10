//! Method and constructor application, overload resolution, and inference.

use super::*;

pub(super) mod constructors;
pub use constructors::ConstructorCandidate;
mod inference;
pub(in crate::typer) mod overload;
pub(in crate::typer) use inference::InferenceLocation;

#[derive(Clone, Copy)]
pub(in crate::typer) struct ApplicationCandidate {
    pub(in crate::typer) symbol: SymbolId,
    pub(in crate::typer) callable: TypeId,
    pub(in crate::typer) member: Option<MemberCandidate>,
    pub(in crate::typer) rejection: Option<OverloadRejection>,
}

pub(in crate::typer) struct ResolvedApplicationFunction {
    pub(in crate::typer) typed: TreeId<Typed>,
    pub(in crate::typer) callable: TypeId,
    pub(in crate::typer) arguments: Vec<TypedArgument>,
    pub(in crate::typer) argument_trees: Option<Vec<TreeId<Untyped>>>,
}

pub(in crate::typer) struct ResolvedApplication {
    pub(in crate::typer) tree_index: u32,
    pub(in crate::typer) application_kind: ApplyKind,
    pub(in crate::typer) argument_trees: Vec<TreeId<Untyped>>,
    pub(in crate::typer) position: Option<SourceSpan>,
    pub(in crate::typer) context: ExpressionContext,
    pub(in crate::typer) function: TreeId<Typed>,
    pub(in crate::typer) callable: TypeId,
    pub(in crate::typer) typed_arguments: Option<Vec<TypedArgument>>,
    pub(in crate::typer) is_constructor_application: bool,
}

pub(in crate::typer) struct InfixApplicationRequest {
    pub(in crate::typer) operator: Name,
    pub(in crate::typer) qualifier: TreeId<Typed>,
    pub(in crate::typer) receiver_type: TypeId,
    pub(in crate::typer) argument_tree: TreeId<Untyped>,
    pub(in crate::typer) context: ExpressionContext,
    pub(in crate::typer) tree_index: u32,
    pub(in crate::typer) position: Option<SourceSpan>,
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct ApplicationRequest<'a> {
    pub(in crate::typer) function_tree: TreeId<Untyped>,
    pub(in crate::typer) argument_trees: &'a [TreeId<Untyped>],
    pub(in crate::typer) application_kind: ApplyKind,
    pub(in crate::typer) context: ExpressionContext,
    pub(in crate::typer) tree_index: u32,
}

#[derive(Clone, Copy)]
pub(in crate::typer) struct TypedArgument {
    pub(in crate::typer) typed: TreeId<Typed>,
    pub(in crate::typer) own_type: TypeId,
    pub(in crate::typer) widened_type: TypeId,
}

/// A selected member cannot be used without source argument syntax.
pub(in crate::typer) enum ZeroArgumentSelectionError {
    Selection(TyperError),
    MethodNeedsArgumentList,
    PolymorphicDeferred,
}

impl SourceTyper<'_> {
    /// Selects a member used without an argument list. Parameterless methods
    /// and values retain their exact selected reference; methods with a clause
    /// and polymorphic results need syntax/inference this call does not supply.
    pub(in crate::typer) fn type_zero_argument_selected_call(
        &mut self,
        tree_index: u32,
        qualifier: TreeId<Typed>,
        name: Name,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, ZeroArgumentSelectionError> {
        let selected = self
            .type_selected_member_on_qualifier(
                tree_index,
                qualifier,
                name,
                false,
                position,
                info_journal,
            )
            .map_err(ZeroArgumentSelectionError::Selection)?;
        let selected_type = self.typed_arena.get(selected).ty;
        let callable = self
            .widen_expression_type_journaled(selected_type, info_journal, 0)
            .map_err(ZeroArgumentSelectionError::Selection)?;
        match self.store.types.try_get(callable) {
            Some(Type::Method(_)) => Err(ZeroArgumentSelectionError::MethodNeedsArgumentList),
            Some(Type::Poly(_)) => Err(ZeroArgumentSelectionError::PolymorphicDeferred),
            _ => Ok(selected),
        }
    }

    pub(in crate::typer) fn type_application(
        &mut self,
        tree: TreeId<Untyped>,
        application: dotty_core::ast::Apply<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let request = ApplicationRequest {
            function_tree: application.function,
            argument_trees: &application.args,
            application_kind: application.kind,
            context,
            tree_index: tree.index(),
        };
        let constructor_function =
            self.resolve_constructor_application_function(request, info_journal, new_mappings)?;
        let is_constructor_application = constructor_function.is_some();
        let resolved_function = if constructor_function.is_some() {
            None
        } else {
            self.resolve_overloaded_application_function(request, info_journal, new_mappings)?
        };
        let (function, callable, typed_arguments, resolved_argument_trees) =
            if let Some((function, callable, arguments)) = constructor_function {
                (function, callable, arguments, None)
            } else if let Some(resolved) = resolved_function {
                (
                    resolved.typed,
                    resolved.callable,
                    Some(resolved.arguments),
                    resolved.argument_trees,
                )
            } else {
                let function = self.type_value_expression_inner(
                    application.function,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let function_type = self.typed_arena.get(function).ty;
                let callable =
                    self.widen_expression_type_journaled(function_type, info_journal, 0)?;
                (function, callable, None, None)
            };
        self.type_resolved_application(
            ResolvedApplication {
                tree_index: tree.index(),
                application_kind: application.kind,
                argument_trees: resolved_argument_trees
                    .unwrap_or_else(|| application.args.to_vec()),
                position,
                context,
                function,
                callable,
                typed_arguments,
                is_constructor_application,
            },
            info_journal,
            new_mappings,
        )
    }

    pub(in crate::typer) fn type_resolved_application(
        &mut self,
        request: ResolvedApplication,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let ResolvedApplication {
            tree_index,
            application_kind,
            argument_trees,
            position,
            context,
            function,
            mut callable,
            mut typed_arguments,
            is_constructor_application,
        } = request;
        if let Some(Type::Poly(poly)) = self.store.types.try_get(callable).cloned() {
            let arguments = if let Some(arguments) = typed_arguments.take() {
                arguments
            } else {
                let mut arguments = Vec::with_capacity(argument_trees.len());
                for argument_tree in &argument_trees {
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
                arguments
            };
            let type_arguments = self.infer_poly_application_arguments(
                callable,
                &poly,
                &arguments,
                application_kind,
                tree_index,
                info_journal,
            )?;
            let instantiated =
                dotty_core::types::instantiate_poly(self.store, callable, &type_arguments)
                    .map_err(TyperError::PolyInstantiation)?;
            for (parameter_index, (argument, bounds)) in type_arguments
                .iter()
                .copied()
                .zip(instantiated.bounds.iter().copied())
                .enumerate()
            {
                self.check_explicit_type_argument_bounds(
                    argument,
                    bounds,
                    parameter_index,
                    tree_index,
                    info_journal,
                )
                .map_err(|error| match error {
                    TyperError::ExplicitTypeArgumentBoundViolation {
                        parameter_index,
                        argument,
                        bound,
                        side,
                        ..
                    } => TyperError::InferredTypeArgumentBoundViolation {
                        source: self.source,
                        tree_index,
                        parameter_index,
                        argument,
                        bound,
                        side,
                    },
                    TyperError::UnsupportedExplicitTypeArgumentBounds {
                        parameter_index,
                        bounds,
                        ..
                    } => TyperError::UnsupportedInferredTypeArgumentBounds {
                        source: self.source,
                        tree_index,
                        parameter_index,
                        argument,
                        bounds,
                    },
                    TyperError::ExplicitTypeArgumentBoundCheckUnsupported {
                        parameter_index,
                        argument,
                        bound,
                        error,
                        ..
                    } => TyperError::InferredTypeArgumentBoundCheckUnsupported {
                        source: self.source,
                        tree_index,
                        parameter_index,
                        argument,
                        bound,
                        error,
                    },
                    error => error,
                })?;
            }
            callable = instantiated.result;
            typed_arguments = Some(arguments);
        }
        let method = match self.store.types.try_get(callable) {
            Some(Type::Method(method)) => method.clone(),
            _ => {
                return Err(TyperError::ApplicationCalleeNotMethod {
                    source: self.source,
                    tree_index,
                    ty: callable,
                });
            }
        };
        if !application_kind_accepts(application_kind, method.kind) {
            return Err(TyperError::ApplicationMethodKindMismatch {
                source: self.source,
                tree_index,
                application_kind,
                method_kind: method.kind,
            });
        }
        if argument_trees.len() != method.params.len() {
            return Err(TyperError::ApplicationArityMismatch {
                source: self.source,
                tree_index,
                expected: method.params.len(),
                actual: argument_trees.len(),
            });
        }
        for (parameter_index, parameter) in method.params.iter().enumerate() {
            if parameter.erased {
                return Err(TyperError::ErasedApplicationParameterDeferred {
                    source: self.source,
                    tree_index,
                    parameter_index,
                });
            }
            if parameter.varargs {
                return Err(TyperError::VarargsApplicationParameterDeferred {
                    source: self.source,
                    tree_index,
                    parameter_index,
                });
            }
        }
        if self.type_contains_param_ref(method.result, callable)? {
            return Err(TyperError::DependentMethodApplicationDeferred {
                source: self.source,
                tree_index,
                binder: callable,
                result: method.result,
            });
        }
        let mut arguments = Vec::with_capacity(argument_trees.len());
        for (argument_index, (argument_tree, parameter)) in
            argument_trees.iter().zip(&method.params).enumerate()
        {
            let expected_type = match self.store.types.try_get(parameter.ty) {
                Some(Type::ByName { result }) => *result,
                _ => parameter.ty,
            };
            let (argument, actual) = if let Some(typed_arguments) = &typed_arguments {
                let typed_argument = typed_arguments[argument_index];
                debug_assert_eq!(
                    self.typed_arena.get(typed_argument.typed).ty,
                    typed_argument.own_type
                );
                let actual = self.adapt_expression_type_to_expected(
                    typed_argument.own_type,
                    expected_type,
                    info_journal,
                )?;
                (typed_argument.typed, actual)
            } else {
                let argument = self.type_value_expression_inner(
                    *argument_tree,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let argument_type = self.typed_arena.get(argument).ty;
                let actual = self.adapt_expression_type_to_expected(
                    argument_type,
                    expected_type,
                    info_journal,
                )?;
                (argument, actual)
            };
            match self.conforms(actual, expected_type) {
                Ok(true) => {}
                Ok(false)
                    if is_constructor_application
                        && self.constructor_argument_types_equivalent(
                            actual,
                            parameter.ty,
                            info_journal,
                            0,
                        )? => {}
                Ok(false) => {
                    return Err(TyperError::ApplicationArgumentTypeMismatch {
                        source: self.source,
                        tree_index,
                        argument_index,
                        actual,
                        expected: expected_type,
                    });
                }
                Err(error) => {
                    return Err(TyperError::ApplicationArgumentConformanceUnsupported {
                        source: self.source,
                        tree_index,
                        argument_index,
                        actual,
                        expected: expected_type,
                        error: Box::new(error),
                    });
                }
            }
            arguments.push(argument);
        }
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply_with_kind(
                function,
                arguments,
                application_kind,
                method.result,
                position,
            ),
        )
    }

    pub(in crate::typer) fn type_type_application(
        &mut self,
        tree: TreeId<Untyped>,
        application: dotty_core::ast::TypeApply<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let function = match self.type_value_expression_inner(
            application.function,
            context,
            info_journal,
            new_mappings,
        ) {
            Ok(function) => function,
            Err(TyperError::OverloadedReferenceDeferred { .. })
            | Err(TyperError::OverloadedSelectionDeferred { .. }) => {
                return Err(TyperError::OverloadedTypeApplicationDeferred {
                    source: self.source,
                    tree_index: tree.index(),
                });
            }
            Err(error) => return Err(error),
        };
        let function_type = self.typed_arena.get(function).ty;
        let callable = self.widen_expression_type_journaled(function_type, info_journal, 0)?;
        let poly = match self.store.types.try_get(callable) {
            Some(Type::Poly(poly)) => poly.clone(),
            _ => {
                return Err(TyperError::ExplicitTypeApplicationCalleeNotPoly {
                    source: self.source,
                    tree_index: tree.index(),
                    ty: callable,
                });
            }
        };

        let mut type_arguments = Vec::with_capacity(application.args.len());
        for argument in &application.args {
            type_arguments.push(self.type_of_tpt_inner_journaled(
                *argument,
                context.lexical,
                info_journal,
            )?);
        }
        if type_arguments.len() != poly.params.len() {
            return Err(TyperError::ExplicitTypeApplicationArityMismatch {
                source: self.source,
                tree_index: tree.index(),
                expected: poly.params.len(),
                actual: type_arguments.len(),
            });
        }

        let instantiated =
            dotty_core::types::instantiate_poly(self.store, callable, &type_arguments)
                .map_err(TyperError::PolyInstantiation)?;
        for (parameter_index, (argument, bounds)) in type_arguments
            .iter()
            .copied()
            .zip(instantiated.bounds.iter().copied())
            .enumerate()
        {
            self.check_explicit_type_argument_bounds(
                argument,
                bounds,
                parameter_index,
                tree.index(),
                info_journal,
            )?;
        }

        let mut typed_type_arguments = Vec::with_capacity(application.args.len());
        for (source_argument, type_argument) in application.args.iter().copied().zip(type_arguments)
        {
            typed_type_arguments.push(self.reify_type_argument(
                source_argument,
                type_argument,
                new_mappings,
            )?);
        }
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).type_apply(
                function,
                typed_type_arguments,
                instantiated.result,
                position,
            ),
        )
    }
}
