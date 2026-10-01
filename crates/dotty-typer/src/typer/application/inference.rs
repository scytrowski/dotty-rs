//! Shared generic argument constraint collection and bound validation.

use super::super::*;

#[derive(Clone, Copy)]
pub(in crate::typer) struct InferenceLocation {
    pub(in crate::typer) tree_index: u32,
    pub(in crate::typer) argument_index: usize,
    pub(in crate::typer) depth: usize,
}

impl SourceTyper<'_> {
    pub(in crate::typer) fn check_explicit_type_argument_bounds(
        &mut self,
        argument: TypeId,
        bounds: TypeId,
        parameter_index: usize,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let mut seen = std::collections::HashSet::new();
        self.complete_relation_type(argument, info_journal, &mut seen, 0)?;
        self.complete_relation_type(bounds, info_journal, &mut seen, 0)?;
        let (low, high) = match self.store.types.try_get(bounds) {
            Some(Type::Bounds { low, high }) => (*low, *high),
            _ => {
                return Err(TyperError::UnsupportedExplicitTypeArgumentBounds {
                    source: self.source,
                    tree_index,
                    parameter_index,
                    bounds,
                });
            }
        };
        match self.conforms(low, argument) {
            Ok(true) => {}
            Ok(false) => {
                return Err(TyperError::ExplicitTypeArgumentBoundViolation {
                    source: self.source,
                    tree_index,
                    parameter_index,
                    argument,
                    bound: low,
                    side: TypeArgumentBoundSide::Lower,
                });
            }
            Err(error) => {
                return Err(TyperError::ExplicitTypeArgumentBoundCheckUnsupported {
                    source: self.source,
                    tree_index,
                    parameter_index,
                    argument,
                    bound: low,
                    error: Box::new(error),
                });
            }
        }
        match self.conforms(argument, high) {
            Ok(true) => Ok(()),
            Ok(false) => Err(TyperError::ExplicitTypeArgumentBoundViolation {
                source: self.source,
                tree_index,
                parameter_index,
                argument,
                bound: high,
                side: TypeArgumentBoundSide::Upper,
            }),
            Err(error) => Err(TyperError::ExplicitTypeArgumentBoundCheckUnsupported {
                source: self.source,
                tree_index,
                parameter_index,
                argument,
                bound: high,
                error: Box::new(error),
            }),
        }
    }

    pub(in crate::typer) fn infer_poly_application_arguments(
        &mut self,
        binder: TypeId,
        poly: &PolyType,
        arguments: &[TypedArgument],
        application_kind: ApplyKind,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Vec<TypeId>, TyperError> {
        let Some(Type::Method(method)) = self.store.types.try_get(poly.result).cloned() else {
            return Err(TyperError::UnsupportedPolymorphicApplicationShape {
                source: self.source,
                tree_index,
                binder,
            });
        };
        if method.params.len() != arguments.len() {
            return Err(TyperError::ApplicationArityMismatch {
                source: self.source,
                tree_index,
                expected: method.params.len(),
                actual: arguments.len(),
            });
        }
        if !application_kind_accepts(application_kind, method.kind)
            || method.params.iter().any(|param| {
                param.erased
                    || param.varargs
                    || matches!(
                        self.store.types.try_get(param.ty),
                        Some(Type::ByName { .. })
                    )
            })
            || self.type_contains_param_ref(poly.result, poly.result)?
        {
            return Err(TyperError::UnsupportedPolymorphicApplicationShape {
                source: self.source,
                tree_index,
                binder,
            });
        }

        let mut inferred = vec![None; poly.params.len()];
        for (argument_index, (parameter, argument)) in
            method.params.iter().zip(arguments).enumerate()
        {
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
            )?;
        }
        let mut type_arguments = Vec::with_capacity(poly.params.len());
        for (parameter_index, inferred_type) in inferred.into_iter().enumerate() {
            let Some(inferred_type) = inferred_type else {
                return Err(TyperError::UnconstrainedTypeParameter {
                    source: self.source,
                    tree_index,
                    binder,
                    parameter_index,
                });
            };
            type_arguments.push(inferred_type);
        }
        Ok(type_arguments)
    }

    pub(in crate::typer) fn infer_type_constraints(
        &mut self,
        formal: TypeId,
        actual: TypeId,
        binder: TypeId,
        inferred: &mut [Option<TypeId>],
        location: InferenceLocation,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let InferenceLocation {
            tree_index,
            argument_index,
            depth,
        } = location;
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Err(TyperError::UnsupportedInferenceShape {
                source: self.source,
                tree_index,
                binder,
                parameter_index: None,
                formal,
                actual,
            });
        }
        let Some(formal_node) = self.store.types.try_get(formal).cloned() else {
            return Err(TyperError::TypeNormalization(
                if self.store.types.contains(formal) {
                    crate::types::TypeNormalizeError::UnfilledType { ty: formal }
                } else {
                    crate::types::TypeNormalizeError::InvalidType { ty: formal }
                },
            ));
        };
        if let Type::ParamRef {
            binder: found,
            index,
        } = formal_node
            && found == binder
        {
            let parameter_index = index as usize;
            let Some(slot) = inferred.get_mut(parameter_index) else {
                return Err(TyperError::UnsupportedInferenceShape {
                    source: self.source,
                    tree_index,
                    binder,
                    parameter_index: Some(parameter_index),
                    formal,
                    actual,
                });
            };
            if let Some(previous) = *slot {
                let mut seen = std::collections::HashSet::new();
                self.complete_relation_type(previous, info_journal, &mut seen, 0)?;
                self.complete_relation_type(actual, info_journal, &mut seen, 0)?;
                let equivalent = match (
                    self.conforms(previous, actual),
                    self.conforms(actual, previous),
                ) {
                    (Ok(true), Ok(true)) => true,
                    (Ok(_), Ok(_)) => false,
                    _ => {
                        return Err(TyperError::UnsupportedInferenceShape {
                            source: self.source,
                            tree_index,
                            binder,
                            parameter_index: Some(parameter_index),
                            formal,
                            actual,
                        });
                    }
                };
                if !equivalent {
                    return Err(TyperError::ConflictingInferenceConstraints {
                        source: self.source,
                        tree_index,
                        binder,
                        parameter_index,
                        first: previous,
                        second: actual,
                    });
                }
            } else {
                *slot = Some(actual);
            }
            return Ok(());
        }

        if let Type::Applied { tycon, args } = formal_node
            && self.type_contains_param_ref(formal, binder)?
        {
            let Some(Type::Applied {
                tycon: actual_tycon,
                args: actual_args,
            }) = self.store.types.try_get(actual).cloned()
            else {
                return Err(TyperError::UnsupportedInferenceShape {
                    source: self.source,
                    tree_index,
                    binder,
                    parameter_index: None,
                    formal,
                    actual,
                });
            };
            let equivalent_constructor = match (
                self.conforms(tycon, actual_tycon),
                self.conforms(actual_tycon, tycon),
            ) {
                (Ok(true), Ok(true)) => true,
                (Ok(_), Ok(_)) => self.same_top_level_type_constructor(tycon, actual_tycon),
                _ => self.same_top_level_type_constructor(tycon, actual_tycon),
            };
            if !equivalent_constructor || args.len() != actual_args.len() {
                return Err(TyperError::UnsupportedInferenceShape {
                    source: self.source,
                    tree_index,
                    binder,
                    parameter_index: None,
                    formal,
                    actual,
                });
            }
            for (formal_argument, actual_argument) in args.into_iter().zip(actual_args) {
                self.infer_type_constraints(
                    formal_argument,
                    actual_argument,
                    binder,
                    inferred,
                    InferenceLocation {
                        depth: depth + 1,
                        ..location
                    },
                    info_journal,
                )?;
            }
            return Ok(());
        }

        if self.type_contains_param_ref(formal, binder)? {
            return Err(TyperError::UnsupportedInferenceShape {
                source: self.source,
                tree_index,
                binder,
                parameter_index: None,
                formal,
                actual,
            });
        }
        let mut seen = std::collections::HashSet::new();
        self.complete_relation_type(formal, info_journal, &mut seen, 0)?;
        self.complete_relation_type(actual, info_journal, &mut seen, 0)?;
        match self.conforms(actual, formal) {
            Ok(true) => Ok(()),
            Ok(false) => Err(TyperError::ApplicationArgumentTypeMismatch {
                source: self.source,
                tree_index,
                argument_index,
                actual,
                expected: formal,
            }),
            Err(_) => Err(TyperError::UnsupportedInferenceShape {
                source: self.source,
                tree_index,
                binder,
                parameter_index: None,
                formal,
                actual,
            }),
        }
    }

    pub(in crate::typer) fn same_top_level_type_constructor(
        &self,
        left: TypeId,
        right: TypeId,
    ) -> bool {
        let (
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(left),
                ..
            }),
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(right),
                ..
            }),
        ) = (
            self.store.types.try_get(left),
            self.store.types.try_get(right),
        )
        else {
            return false;
        };
        if left != right || !self.store.symbols.contains(*left) {
            return false;
        }
        let owner = self.store.symbols.get(*left).owner;
        owner.is_some_and(|owner| {
            self.store.symbols.contains(owner)
                && self.store.symbols.get(owner).kind == SymbolKind::Package
        })
    }

    pub(in crate::typer) fn constructor_argument_types_equivalent(
        &mut self,
        left: TypeId,
        right: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        depth: usize,
    ) -> Result<bool, TyperError> {
        if left == right {
            return Ok(true);
        }
        if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
            return Ok(false);
        }
        let (
            Some(Type::Applied {
                tycon: left_tycon,
                args: left_args,
            }),
            Some(Type::Applied {
                tycon: right_tycon,
                args: right_args,
            }),
        ) = (
            self.store.types.try_get(left).cloned(),
            self.store.types.try_get(right).cloned(),
        )
        else {
            return Ok(false);
        };
        if left_args.len() != right_args.len()
            || !self.same_top_level_type_constructor(left_tycon, right_tycon)
        {
            return Ok(false);
        }
        for (left_arg, right_arg) in left_args.into_iter().zip(right_args) {
            if left_arg == right_arg {
                continue;
            }
            let mut seen = HashSet::new();
            self.complete_relation_type(left_arg, info_journal, &mut seen, 0)?;
            self.complete_relation_type(right_arg, info_journal, &mut seen, 0)?;
            match (
                self.conforms(left_arg, right_arg),
                self.conforms(right_arg, left_arg),
            ) {
                (Ok(true), Ok(true)) => {}
                (Ok(false), Ok(false))
                    if self.constructor_argument_types_equivalent(
                        left_arg,
                        right_arg,
                        info_journal,
                        depth + 1,
                    )? => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    pub(in crate::typer) fn type_contains_param_ref(
        &self,
        root: TypeId,
        binder: TypeId,
    ) -> Result<bool, TyperError> {
        fn visit(
            typer: &SourceTyper<'_>,
            ty: TypeId,
            binder: TypeId,
            depth: usize,
            seen: &mut std::collections::HashSet<TypeId>,
        ) -> Result<bool, TyperError> {
            if depth >= crate::types::MAX_TYPE_NORMALIZATION_DEPTH {
                return Err(TyperError::TypeNormalization(
                    crate::types::TypeNormalizeError::TooDeep,
                ));
            }
            if !seen.insert(ty) {
                return Ok(false);
            }
            let Some(node) = typer.store.types.try_get(ty) else {
                return Err(TyperError::TypeNormalization(
                    if typer.store.types.contains(ty) {
                        crate::types::TypeNormalizeError::UnfilledType { ty }
                    } else {
                        crate::types::TypeNormalizeError::InvalidType { ty }
                    },
                ));
            };
            if matches!(node, Type::ParamRef { binder: found, .. } if *found == binder) {
                return Ok(true);
            }
            let mut children = Vec::new();
            match node {
                Type::TermRef { prefix, .. } | Type::TypeRef { prefix, .. } => {
                    children.push(*prefix);
                }
                Type::SuperType {
                    this_type,
                    super_type,
                } => children.extend([*this_type, *super_type]),
                Type::Applied { tycon, args } => {
                    children.push(*tycon);
                    children.extend(args.iter().copied());
                }
                Type::Bounds { low, high } => children.extend([*low, *high]),
                Type::AliasingBounds { alias }
                | Type::ByName { result: alias }
                | Type::Flexible { underlying: alias }
                | Type::Recursive { parent: alias }
                | Type::Wildcard { bounds: alias }
                | Type::JavaArray { element: alias }
                | Type::Repeated { element: alias } => children.push(*alias),
                Type::And { left, right } | Type::Or { left, right } => {
                    children.extend([*left, *right]);
                }
                Type::Refined { parent, info, .. } => children.extend([*parent, *info]),
                Type::Method(method) => {
                    children.extend(method.params.iter().map(|parameter| parameter.ty));
                    children.push(method.result);
                }
                Type::Poly(poly) => {
                    children.extend(poly.params.iter().map(|parameter| parameter.bounds));
                    children.push(poly.result);
                }
                Type::TypeLambda(lambda) => {
                    children.extend(lambda.params.iter().map(|parameter| parameter.bounds));
                    children.push(lambda.result);
                }
                Type::MatchCase { pattern, result } => children.extend([*pattern, *result]),
                Type::Annotated { underlying, .. } => children.push(*underlying),
                Type::ParamRef { .. }
                | Type::NoType
                | Type::Error(_)
                | Type::NoPrefix
                | Type::ThisType { .. }
                | Type::Constant(_)
                | Type::RecThis { .. } => {}
                Type::Match(match_type) => {
                    children.extend([match_type.bound, match_type.scrutinee]);
                    children.extend(match_type.cases.iter().copied());
                }
                Type::ClassInfo(info) => {
                    children.push(info.prefix);
                    children.extend(info.parents.iter().copied());
                    children.extend(info.self_type);
                }
            }
            for child in children {
                if visit(typer, child, binder, depth + 1, seen)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        visit(self, root, binder, 0, &mut std::collections::HashSet::new())
    }
}
