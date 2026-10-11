//! Bounded operations on the outermost semantic type.
//!
//! These helpers never complete symbols or perform subtyping/member lookup.
//! Opening an applied generic alias allocates a binder-substituted result via
//! `dotty-core`; failed normalization rolls back those allocations. Other
//! child types are not recursively normalized.

use std::fmt;

use dotty_core::types::{Type, TypeRebindError, TypeRefTarget, instantiate_type_lambda};
use dotty_core::{SemanticStore, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, TypeId};

/// Maximum number of alias/proxy links followed by one normalization request.
pub const MAX_TYPE_NORMALIZATION_DEPTH: usize = 256;

/// The symbol-info state reported when normalization cannot read a declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolInfoState {
    Missing,
    Deferred,
    Error,
}

/// A soundness limitation or malformed reference encountered during
/// top-level type normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeNormalizeError {
    /// A `TypeId` is a reserved slot that has not been filled.
    UnfilledType { ty: TypeId },
    /// A `TypeId` points outside the semantic store's type arena.
    InvalidType { ty: TypeId },
    /// A symbol reference points outside this semantic store.
    UnknownSymbol { symbol: SymbolId },
    /// A type alias is opaque at this stage; its representation is hidden.
    OpaqueAlias { symbol: SymbolId },
    /// A type alias needs type-parameter substitution before it can be opened.
    AliasRequiresSubstitution { symbol: SymbolId },
    /// An applied alias was supplied a different number of arguments than its binder requires.
    AliasArgumentArityMismatch {
        symbol: SymbolId,
        expected: usize,
        actual: usize,
    },
    /// Instantiating a structurally malformed alias failed in the shared rebinder.
    AliasInstantiationFailed {
        symbol: SymbolId,
        error: TypeRebindError,
    },
    /// An alias declaration has no complete semantic info.
    AliasInfoIncomplete {
        symbol: SymbolId,
        state: SymbolInfoState,
    },
    /// A completed alias does not have the required `AliasingBounds` shape.
    AliasInfoNotAliasingBounds { symbol: SymbolId, info: TypeId },
    /// Following aliases revisited a declaration symbol.
    AliasCycle { symbol: SymbolId },
    /// A malformed alias/proxy cycle revisited the same outer type node.
    NormalizationCycle { ty: TypeId },
    /// The alias chain or wrapper path exceeds the configured bound.
    TooDeep,
    /// A term reference uses a target represented by a structural name.
    NameTargetCannotBeWidened { ty: TypeId },
    /// A term reference designates a symbol category that is not a term value.
    TermRefKindCannotBeWidened { symbol: SymbolId, kind: SymbolKind },
    /// A term reference's symbol info is not complete.
    TermRefInfoIncomplete {
        symbol: SymbolId,
        state: SymbolInfoState,
    },
    /// The input to term-reference widening is not a `TermRef`.
    NotTermRef { ty: TypeId },
}

impl fmt::Display for TypeNormalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TypeNormalizeError {}

/// Bounded semantic type normalization over an existing store.
pub struct TypeNormalizer<'a> {
    store: &'a mut SemanticStore,
}

impl<'a> TypeNormalizer<'a> {
    /// Creates a normalizer over the supplied semantic store.
    pub const fn new(store: &'a mut SemanticStore) -> Self {
        Self { store }
    }

    /// Opens a chain of non-opaque type aliases at the top level.
    ///
    /// Name-target references, non-alias symbols, and non-alias type forms are
    /// returned unchanged. Alias declarations must already have complete
    /// `AliasingBounds` info. An unapplied generic alias is rejected, while an
    /// applied alias is opened by substituting its exact argument list into
    /// its `TypeLambda` binder.
    pub fn dealias_top(&mut self, ty: TypeId) -> Result<TypeId, TypeNormalizeError> {
        let checkpoint = self.store.checkpoint();
        match self.dealias_top_with_budget(ty, MAX_TYPE_NORMALIZATION_DEPTH) {
            Ok((ty, _)) => Ok(ty),
            Err(error) => {
                self.store.rollback_to(checkpoint);
                Err(error)
            }
        }
    }

    fn dealias_top_with_budget(
        &mut self,
        ty: TypeId,
        budget: usize,
    ) -> Result<(TypeId, usize), TypeNormalizeError> {
        let mut current = ty;
        let mut visited = Vec::new();

        for links in 0..=budget {
            let (symbol, next) = match self.type_at(current)?.clone() {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } => {
                    let Some(next) = self.alias_target(symbol)? else {
                        return Ok((current, links));
                    };
                    if let Type::TypeLambda(lambda) = self.type_at(next)?.clone() {
                        if !lambda.params.is_empty() {
                            return Err(TypeNormalizeError::AliasRequiresSubstitution { symbol });
                        }
                        let instantiated =
                            instantiate_type_lambda(self.store, next, &[]).map_err(|error| {
                                TypeNormalizeError::AliasInstantiationFailed { symbol, error }
                            })?;
                        (symbol, instantiated.result)
                    } else {
                        (symbol, next)
                    }
                }
                Type::Applied { tycon, args } => {
                    let Some(symbol) = self.alias_symbol(tycon)? else {
                        return Ok((current, links));
                    };
                    let Some(next) = self.alias_application_target(symbol, &args)? else {
                        return Ok((current, links));
                    };
                    (symbol, next)
                }
                _ => return Ok((current, links)),
            };
            if visited.contains(&symbol) {
                return Err(TypeNormalizeError::AliasCycle { symbol });
            }
            if links == budget {
                return Err(TypeNormalizeError::TooDeep);
            }
            visited.push(symbol);
            current = next;
        }

        Err(TypeNormalizeError::TooDeep)
    }

    /// Replaces a top-level reference to a value-like term with its complete
    /// symbol info. The referenced symbol is never completed by this method.
    ///
    /// Only `Field`, `Value`, `Variable`, and `Parameter` symbols are widened;
    /// methods, constructors, objects, packages, locals, and name targets need
    /// different typing rules and are rejected.
    pub fn widen_term_ref(&self, ty: TypeId) -> Result<TypeId, TypeNormalizeError> {
        let Type::TermRef { target, .. } = self.type_at(ty)? else {
            return Err(TypeNormalizeError::NotTermRef { ty });
        };
        let symbol = match target {
            dotty_core::types::TermRefTarget::Name(_) => {
                return Err(TypeNormalizeError::NameTargetCannotBeWidened { ty });
            }
            dotty_core::types::TermRefTarget::Symbol(symbol) => *symbol,
        };
        if !self.store.symbols.contains(symbol) {
            return Err(TypeNormalizeError::UnknownSymbol { symbol });
        }
        let declaration = self.store.symbols.get(symbol);
        if !matches!(
            declaration.kind,
            SymbolKind::Field | SymbolKind::Value | SymbolKind::Variable | SymbolKind::Parameter
        ) {
            return Err(TypeNormalizeError::TermRefKindCannotBeWidened {
                symbol,
                kind: declaration.kind,
            });
        }
        match declaration.info {
            SymbolInfo::Complete(info) => {
                self.type_at(info)?;
                Ok(info)
            }
            SymbolInfo::Missing => Err(TypeNormalizeError::TermRefInfoIncomplete {
                symbol,
                state: SymbolInfoState::Missing,
            }),
            SymbolInfo::Deferred(_) => Err(TypeNormalizeError::TermRefInfoIncomplete {
                symbol,
                state: SymbolInfoState::Deferred,
            }),
            SymbolInfo::Error => Err(TypeNormalizeError::TermRefInfoIncomplete {
                symbol,
                state: SymbolInfoState::Error,
            }),
        }
    }

    /// Dealiases and unwraps `Annotated` / `Flexible` at the outermost level.
    ///
    /// Non-alias `Applied` types and all other child types remain intact. An
    /// applied alias is instantiated before returning. Alias links and
    /// wrappers share one depth budget, and repeated outer nodes are reported
    /// as malformed cycles.
    pub fn normalize_for_lookup(&mut self, ty: TypeId) -> Result<TypeId, TypeNormalizeError> {
        let checkpoint = self.store.checkpoint();
        match self.normalize_for_lookup_inner(ty) {
            Ok(normalized) => Ok(normalized),
            Err(error) => {
                self.store.rollback_to(checkpoint);
                Err(error)
            }
        }
    }

    fn normalize_for_lookup_inner(&mut self, ty: TypeId) -> Result<TypeId, TypeNormalizeError> {
        let mut current = ty;
        let mut visited = Vec::new();
        let mut depth = 0;

        loop {
            if visited.contains(&current) {
                return Err(TypeNormalizeError::NormalizationCycle { ty: current });
            }
            visited.push(current);
            let current_type = self.type_at(current)?.clone();
            let (dealiased, links) = self.dealias_top_with_budget(
                current,
                MAX_TYPE_NORMALIZATION_DEPTH.saturating_sub(depth),
            )?;
            depth += links;
            if dealiased != current {
                current = dealiased;
                continue;
            }
            if matches!(current_type, Type::Applied { .. }) {
                return Ok(current);
            }
            let underlying = match self.type_at(current)? {
                Type::Annotated { underlying, .. } | Type::Flexible { underlying } => *underlying,
                _ => return Ok(current),
            };
            if depth == MAX_TYPE_NORMALIZATION_DEPTH {
                return Err(TypeNormalizeError::TooDeep);
            }
            depth += 1;
            current = underlying;
        }
    }

    fn alias_symbol(&self, ty: TypeId) -> Result<Option<SymbolId>, TypeNormalizeError> {
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } = self.type_at(ty)?
        else {
            return Ok(None);
        };
        if !self.store.symbols.contains(*symbol) {
            return Err(TypeNormalizeError::UnknownSymbol { symbol: *symbol });
        }
        Ok(
            matches!(self.store.symbols.get(*symbol).kind, SymbolKind::TypeAlias)
                .then_some(*symbol),
        )
    }

    fn alias_application_target(
        &mut self,
        symbol: SymbolId,
        arguments: &[TypeId],
    ) -> Result<Option<TypeId>, TypeNormalizeError> {
        let Some(target) = self.alias_target(symbol)? else {
            return Ok(None);
        };
        let lambda_arity = match self.type_at(target)?.clone() {
            Type::TypeLambda(lambda) => lambda.params.len(),
            _ if arguments.is_empty() => return Ok(Some(target)),
            _ => {
                return Err(TypeNormalizeError::AliasArgumentArityMismatch {
                    symbol,
                    expected: 0,
                    actual: arguments.len(),
                });
            }
        };
        if arguments.len() != lambda_arity {
            return Err(TypeNormalizeError::AliasArgumentArityMismatch {
                symbol,
                expected: lambda_arity,
                actual: arguments.len(),
            });
        }
        instantiate_type_lambda(self.store, target, arguments)
            .map(|instantiated| Some(instantiated.result))
            .map_err(|error| TypeNormalizeError::AliasInstantiationFailed { symbol, error })
    }

    fn alias_target(&self, symbol: SymbolId) -> Result<Option<TypeId>, TypeNormalizeError> {
        if !self.store.symbols.contains(symbol) {
            return Err(TypeNormalizeError::UnknownSymbol { symbol });
        }
        let declaration = self.store.symbols.get(symbol);
        if declaration.kind != SymbolKind::TypeAlias {
            return Ok(None);
        }
        if declaration.flags.contains(SymbolFlags::OPAQUE) {
            return Err(TypeNormalizeError::OpaqueAlias { symbol });
        }
        let info = match declaration.info {
            SymbolInfo::Complete(info) => info,
            SymbolInfo::Missing => {
                return Err(TypeNormalizeError::AliasInfoIncomplete {
                    symbol,
                    state: SymbolInfoState::Missing,
                });
            }
            SymbolInfo::Deferred(_) => {
                return Err(TypeNormalizeError::AliasInfoIncomplete {
                    symbol,
                    state: SymbolInfoState::Deferred,
                });
            }
            SymbolInfo::Error => {
                return Err(TypeNormalizeError::AliasInfoIncomplete {
                    symbol,
                    state: SymbolInfoState::Error,
                });
            }
        };
        match self.type_at(info)? {
            Type::AliasingBounds { alias } => Ok(Some(*alias)),
            _ => Err(TypeNormalizeError::AliasInfoNotAliasingBounds { symbol, info }),
        }
    }

    fn type_at(&self, ty: TypeId) -> Result<&Type, TypeNormalizeError> {
        self.store.types.try_get(ty).ok_or_else(|| {
            if self.store.types.contains(ty) {
                TypeNormalizeError::UnfilledType { ty }
            } else {
                TypeNormalizeError::InvalidType { ty }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::types::{TermRefTarget, Type, TypeLambda, TypeParam};
    use dotty_core::{Name, Namespace, Symbol, SymbolFlags, SymbolLinks, SymbolOrigin, Visibility};

    struct World {
        store: SemanticStore,
        prefix: TypeId,
    }

    impl World {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let prefix = store.types.alloc(Type::NoPrefix);
            Self { store, prefix }
        }

        fn symbol(
            &mut self,
            text: &str,
            kind: SymbolKind,
            flags: SymbolFlags,
            info: SymbolInfo,
        ) -> SymbolId {
            let name = Name::new(self.store.names.intern(text), Namespace::Type);
            self.store.symbols.alloc(Symbol {
                name,
                owner: None,
                kind,
                flags,
                info,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                visibility: Visibility::Public,
                links: SymbolLinks::default(),
            })
        }

        fn type_ref(&mut self, symbol: SymbolId) -> TypeId {
            self.store.types.alloc(Type::TypeRef {
                prefix: self.prefix,
                target: TypeRefTarget::Symbol(symbol),
            })
        }

        fn alias(&mut self, text: &str, target: TypeId, flags: SymbolFlags) -> (SymbolId, TypeId) {
            let info = self
                .store
                .types
                .alloc(Type::AliasingBounds { alias: target });
            let symbol = self.symbol(
                text,
                SymbolKind::TypeAlias,
                flags,
                SymbolInfo::Complete(info),
            );
            let reference = self.type_ref(symbol);
            (symbol, reference)
        }

        fn generic_alias(&mut self, text: &str, target: SymbolId) -> (SymbolId, TypeId) {
            let reserved = self.store.types.reserve();
            let binder = reserved.id();
            let leaf = self.store.types.alloc(Type::NoType);
            let bounds = self.store.types.alloc(Type::Bounds {
                low: leaf,
                high: leaf,
            });
            let parameter = TypeParam {
                name: dotty_core::TypeName::new(self.store.names.intern("A")),
                bounds,
                declared_variance: None,
            };
            let param_ref = self.store.types.alloc(Type::ParamRef { binder, index: 0 });
            let tycon = self.type_ref(target);
            let body = self.store.types.alloc(Type::Applied {
                tycon,
                args: vec![param_ref],
            });
            let lambda = self.store.types.fill(
                reserved,
                Type::TypeLambda(TypeLambda {
                    params: vec![parameter],
                    result: body,
                }),
            );
            let info = self
                .store
                .types
                .alloc(Type::AliasingBounds { alias: lambda });
            let symbol = self.symbol(
                text,
                SymbolKind::TypeAlias,
                SymbolFlags::EMPTY,
                SymbolInfo::Complete(info),
            );
            let reference = self.type_ref(symbol);
            (symbol, reference)
        }

        fn generic_alias_lambda(&self, symbol: SymbolId) -> TypeId {
            let SymbolInfo::Complete(info) = self.store.symbols.get(symbol).info else {
                panic!("fixture alias should have complete info")
            };
            let Type::AliasingBounds { alias } = self.store.types.get(info) else {
                panic!("fixture alias should use AliasingBounds")
            };
            *alias
        }

        fn set_generic_alias_result(&mut self, symbol: SymbolId, result: TypeId) {
            let lambda_id = self.generic_alias_lambda(symbol);
            let Type::TypeLambda(lambda) = self.store.types.get(lambda_id).clone() else {
                panic!("fixture alias should use a TypeLambda")
            };
            *self.store.types.get_mut(lambda_id) =
                Type::TypeLambda(TypeLambda { result, ..lambda });
        }
    }

    #[test]
    fn dealiases_a_simple_complete_alias_without_allocating() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (_, reference) = w.alias("A", leaf, SymbolFlags::EMPTY);
        let marker = w.store.types.alloc(Type::NoType);
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reference),
            Ok(leaf)
        );
        let next = w.store.types.alloc(Type::NoType);
        assert_eq!(next.index(), marker.index() + 1);
    }

    #[test]
    fn follows_a_chain_of_simple_aliases() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (_, inner) = w.alias("B", leaf, SymbolFlags::EMPTY);
        let (_, outer) = w.alias("A", inner, SymbolFlags::EMPTY);
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(outer),
            Ok(leaf)
        );
    }

    #[test]
    fn leaves_non_alias_symbol_and_name_targets_unchanged() {
        let mut w = World::new();
        let class = w.symbol(
            "C",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let class_ref = w.type_ref(class);
        let name = dotty_core::TypeName::new(w.store.names.intern("T"));
        let name_ref = w.store.types.alloc(Type::TypeRef {
            prefix: w.prefix,
            target: TypeRefTarget::Name(name),
        });
        let mut normalizer = TypeNormalizer::new(&mut w.store);
        assert_eq!(normalizer.dealias_top(class_ref), Ok(class_ref));
        assert_eq!(normalizer.dealias_top(name_ref), Ok(name_ref));
    }

    #[test]
    fn rejects_opaque_aliases_without_opening_them() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (symbol, reference) = w.alias("Opaque", leaf, SymbolFlags::OPAQUE);
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reference),
            Err(TypeNormalizeError::OpaqueAlias { symbol })
        );
    }

    #[test]
    fn rejects_aliases_without_complete_info() {
        for (info, state) in [
            (SymbolInfo::Missing, SymbolInfoState::Missing),
            (SymbolInfo::Error, SymbolInfoState::Error),
        ] {
            let mut w = World::new();
            let symbol = w.symbol("A", SymbolKind::TypeAlias, SymbolFlags::EMPTY, info);
            let reference = w.type_ref(symbol);
            assert_eq!(
                TypeNormalizer::new(&mut w.store).dealias_top(reference),
                Err(TypeNormalizeError::AliasInfoIncomplete { symbol, state })
            );
        }
    }

    #[test]
    fn rejects_alias_info_without_aliasing_bounds() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::Bounds {
            low: w.prefix,
            high: w.prefix,
        });
        let symbol = w.symbol(
            "A",
            SymbolKind::TypeAlias,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = w.type_ref(symbol);
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reference),
            Err(TypeNormalizeError::AliasInfoNotAliasingBounds { symbol, info })
        );
    }

    #[test]
    fn normalizes_applied_generic_alias_but_keeps_bare_substitution_guard() {
        let mut w = World::new();
        let target = w.symbol(
            "List",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let (symbol, reference) = w.generic_alias("Box", target);
        let argument = w.store.types.alloc(Type::NoType);
        let applied = w.store.types.alloc(Type::Applied {
            tycon: reference,
            args: vec![argument],
        });
        let normalized = TypeNormalizer::new(&mut w.store)
            .dealias_top(applied)
            .unwrap();
        let Type::Applied { tycon, args } = w.store.types.get(normalized) else {
            panic!("generic alias should normalize to its target application")
        };
        assert!(matches!(
            w.store.types.get(*tycon),
            Type::TypeRef { target: TypeRefTarget::Symbol(found), .. } if *found == target
        ));
        assert_eq!(args, &[argument]);
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reference),
            Err(TypeNormalizeError::AliasRequiresSubstitution { symbol })
        );
        for arguments in [vec![], vec![argument, argument]] {
            let malformed = w.store.types.alloc(Type::Applied {
                tycon: reference,
                args: arguments.clone(),
            });
            let checkpoint = w.store.checkpoint();
            assert_eq!(
                TypeNormalizer::new(&mut w.store).dealias_top(malformed),
                Err(TypeNormalizeError::AliasArgumentArityMismatch {
                    symbol,
                    expected: 1,
                    actual: arguments.len(),
                })
            );
            assert_eq!(w.store.checkpoint(), checkpoint);
        }
    }

    #[test]
    fn normalizes_a_nested_generic_alias_chain_by_binder_substitution() {
        let mut w = World::new();
        let target = w.symbol(
            "List",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let (inner, _) = w.generic_alias("Inner", target);
        let (_, outer_ref) = w.generic_alias("Outer", inner);
        let argument = w.store.types.alloc(Type::NoType);
        let applied = w.store.types.alloc(Type::Applied {
            tycon: outer_ref,
            args: vec![argument],
        });

        let normalized = TypeNormalizer::new(&mut w.store)
            .normalize_for_lookup(applied)
            .unwrap();

        let Type::Applied { tycon, args } = w.store.types.get(normalized) else {
            panic!("a generic target application should remain applied")
        };
        assert!(matches!(
            w.store.types.get(*tycon),
            Type::TypeRef { target: TypeRefTarget::Symbol(found), .. } if *found == target
        ));
        assert_eq!(args, &[argument]);

        let normalized_again = TypeNormalizer::new(&mut w.store)
            .normalize_for_lookup(applied)
            .unwrap();
        assert_eq!(
            w.store.types.get(normalized),
            w.store.types.get(normalized_again)
        );
    }

    #[test]
    fn reports_direct_and_mutual_applied_alias_cycles_without_leaking_allocations() {
        let mut w = World::new();
        let target = w.symbol(
            "List",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let (direct, direct_ref) = w.generic_alias("Direct", target);
        let direct_lambda = w.generic_alias_lambda(direct);
        let direct_param = w.store.types.alloc(Type::ParamRef {
            binder: direct_lambda,
            index: 0,
        });
        let direct_body = w.store.types.alloc(Type::Applied {
            tycon: direct_ref,
            args: vec![direct_param],
        });
        w.set_generic_alias_result(direct, direct_body);
        let argument = w.store.types.alloc(Type::NoType);
        let direct_application = w.store.types.alloc(Type::Applied {
            tycon: direct_ref,
            args: vec![argument],
        });
        let checkpoint = w.store.checkpoint();
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(direct_application),
            Err(TypeNormalizeError::AliasCycle { symbol: direct })
        );
        assert_eq!(w.store.checkpoint(), checkpoint);

        let (first, first_ref) = w.generic_alias("First", target);
        let (second, second_ref) = w.generic_alias("Second", target);
        let first_lambda = w.generic_alias_lambda(first);
        let second_lambda = w.generic_alias_lambda(second);
        let first_arg = w.store.types.alloc(Type::ParamRef {
            binder: first_lambda,
            index: 0,
        });
        let second_arg = w.store.types.alloc(Type::ParamRef {
            binder: second_lambda,
            index: 0,
        });
        let first_body = w.store.types.alloc(Type::Applied {
            tycon: second_ref,
            args: vec![first_arg],
        });
        let second_body = w.store.types.alloc(Type::Applied {
            tycon: first_ref,
            args: vec![second_arg],
        });
        w.set_generic_alias_result(first, first_body);
        w.set_generic_alias_result(second, second_body);
        let cycle = w.store.types.alloc(Type::Applied {
            tycon: first_ref,
            args: vec![argument],
        });
        let checkpoint = w.store.checkpoint();
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(cycle),
            Err(TypeNormalizeError::AliasCycle { symbol: first })
        );
        assert_eq!(w.store.checkpoint(), checkpoint);
    }

    #[test]
    fn reports_malformed_applied_alias_binders_with_a_focused_error() {
        let mut w = World::new();
        let target = w.symbol(
            "List",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let (symbol, reference) = w.generic_alias("Broken", target);
        let lambda = w.generic_alias_lambda(symbol);
        let bad_reference = w.store.types.alloc(Type::ParamRef {
            binder: lambda,
            index: 3,
        });
        w.set_generic_alias_result(symbol, bad_reference);
        let argument = w.store.types.alloc(Type::NoType);
        let applied = w.store.types.alloc(Type::Applied {
            tycon: reference,
            args: vec![argument],
        });
        let checkpoint = w.store.checkpoint();

        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(applied),
            Err(TypeNormalizeError::AliasInstantiationFailed {
                symbol,
                error: TypeRebindError::InvalidTypeLambdaParamRef {
                    binder: lambda,
                    index: 3,
                    arity: 1,
                },
            })
        );
        assert_eq!(w.store.checkpoint(), checkpoint);
    }

    #[test]
    fn rejects_unfilled_nodes_inside_an_applied_alias_and_rolls_back() {
        let mut w = World::new();
        let target = w.symbol(
            "List",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let (symbol, reference) = w.generic_alias("Incomplete", target);
        let lambda = w.generic_alias_lambda(symbol);
        let unfilled = w.store.types.reserve().id();
        w.set_generic_alias_result(symbol, unfilled);
        let argument = w.store.types.alloc(Type::NoType);
        let applied = w.store.types.alloc(Type::Applied {
            tycon: reference,
            args: vec![argument],
        });
        let checkpoint = w.store.checkpoint();

        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(applied),
            Err(TypeNormalizeError::AliasInstantiationFailed {
                symbol,
                error: TypeRebindError::UnfilledType { id: unfilled },
            })
        );
        assert_eq!(w.store.checkpoint(), checkpoint);
        assert!(matches!(
            w.store.types.get(lambda),
            Type::TypeLambda(TypeLambda { params, .. }) if params.len() == 1
        ));
    }

    #[test]
    fn reports_alias_cycles() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "A",
            SymbolKind::TypeAlias,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = w.type_ref(symbol);
        *w.store.types.get_mut(info) = Type::AliasingBounds { alias: reference };
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reference),
            Err(TypeNormalizeError::AliasCycle { symbol })
        );
    }

    #[test]
    fn rejects_alias_chains_beyond_the_normalization_limit() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let mut current = leaf;
        let mut outer = leaf;
        for index in 0..=MAX_TYPE_NORMALIZATION_DEPTH {
            let (_, reference) = w.alias(&format!("A{index}"), current, SymbolFlags::EMPTY);
            current = reference;
            outer = reference;
        }
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(outer),
            Err(TypeNormalizeError::TooDeep)
        );
    }

    #[test]
    fn accepts_alias_chains_at_the_normalization_limit() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let mut current = leaf;
        for index in 0..MAX_TYPE_NORMALIZATION_DEPTH {
            current = w.alias(&format!("A{index}"), current, SymbolFlags::EMPTY).1;
        }
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(current),
            Ok(leaf)
        );
    }

    #[test]
    fn reports_an_unfilled_type_slot_instead_of_reading_it() {
        let mut w = World::new();
        let reserved = w.store.types.reserve();
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reserved.id()),
            Err(TypeNormalizeError::UnfilledType { ty: reserved.id() })
        );
    }

    #[test]
    fn reports_an_out_of_range_root_type_id() {
        let mut foreign_store = SemanticStore::new();
        let foreign_ty = foreign_store.types.alloc(Type::NoType);
        let mut empty_store = SemanticStore::new();

        assert_eq!(
            TypeNormalizer::new(&mut empty_store).dealias_top(foreign_ty),
            Err(TypeNormalizeError::InvalidType { ty: foreign_ty })
        );
    }

    #[test]
    fn reports_an_out_of_range_nested_proxy_reference() {
        let mut w = World::new();
        let mut foreign_store = SemanticStore::new();
        foreign_store.types.alloc(Type::NoType);
        foreign_store.types.alloc(Type::NoType);
        let foreign_ty = foreign_store.types.alloc(Type::NoType);
        let flexible = w.store.types.alloc(Type::Flexible {
            underlying: foreign_ty,
        });

        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(flexible),
            Err(TypeNormalizeError::InvalidType { ty: foreign_ty })
        );
    }

    #[test]
    fn term_name_targets_are_valid_types_but_not_aliases() {
        let mut w = World::new();
        let term = dotty_core::TermName::new(w.store.names.intern("x"));
        let reference = w.store.types.alloc(Type::TermRef {
            prefix: w.prefix,
            target: TermRefTarget::Name(term),
        });
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(reference),
            Ok(reference)
        );
    }

    fn term_ref(w: &mut World, symbol: SymbolId) -> TypeId {
        w.store.types.alloc(Type::TermRef {
            prefix: w.prefix,
            target: TermRefTarget::Symbol(symbol),
        })
    }

    #[test]
    fn widens_a_complete_field_reference_without_allocating() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "field",
            SymbolKind::Field,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);
        let marker = w.store.types.alloc(Type::NoType);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Ok(info)
        );
        assert_eq!(
            w.store.types.alloc(Type::NoType).index(),
            marker.index() + 1
        );
    }

    #[test]
    fn widens_a_complete_value_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "value",
            SymbolKind::Value,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Ok(info)
        );
    }

    #[test]
    fn widens_a_complete_variable_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "variable",
            SymbolKind::Variable,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Ok(info)
        );
    }

    #[test]
    fn widens_a_complete_parameter_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "parameter",
            SymbolKind::Parameter,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Ok(info)
        );
    }

    #[test]
    fn does_not_widen_a_method_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "method",
            SymbolKind::Method,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Err(TypeNormalizeError::TermRefKindCannotBeWidened {
                symbol,
                kind: SymbolKind::Method
            })
        );
    }

    #[test]
    fn does_not_widen_an_object_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "object",
            SymbolKind::Object,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Err(TypeNormalizeError::TermRefKindCannotBeWidened {
                symbol,
                kind: SymbolKind::Object
            })
        );
    }

    #[test]
    fn does_not_widen_a_constructor_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "<init>",
            SymbolKind::Constructor,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Err(TypeNormalizeError::TermRefKindCannotBeWidened {
                symbol,
                kind: SymbolKind::Constructor
            })
        );
    }

    #[test]
    fn does_not_widen_a_package_reference() {
        let mut w = World::new();
        let info = w.store.types.alloc(Type::NoType);
        let symbol = w.symbol(
            "package",
            SymbolKind::Package,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(info),
        );
        let reference = term_ref(&mut w, symbol);

        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Err(TypeNormalizeError::TermRefKindCannotBeWidened {
                symbol,
                kind: SymbolKind::Package
            })
        );
    }

    #[test]
    fn does_not_widen_a_name_target() {
        let mut w = World::new();
        let name = dotty_core::TermName::new(w.store.names.intern("member"));
        let reference = w.store.types.alloc(Type::TermRef {
            prefix: w.prefix,
            target: TermRefTarget::Name(name),
        });
        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
            Err(TypeNormalizeError::NameTargetCannotBeWidened { ty: reference })
        );
    }

    #[test]
    fn reports_missing_and_error_term_info_without_completing_symbols() {
        for info in [SymbolInfo::Missing, SymbolInfo::Error] {
            let mut w = World::new();
            let symbol = w.symbol("value", SymbolKind::Value, SymbolFlags::EMPTY, info);
            let reference = term_ref(&mut w, symbol);
            let state = match info {
                SymbolInfo::Missing => SymbolInfoState::Missing,
                SymbolInfo::Error => SymbolInfoState::Error,
                _ => unreachable!(),
            };
            assert_eq!(
                TypeNormalizer::new(&mut w.store).widen_term_ref(reference),
                Err(TypeNormalizeError::TermRefInfoIncomplete { symbol, state })
            );
            assert_eq!(*w.store.symbols.info(symbol), info);
        }
    }

    #[test]
    fn rejects_non_term_ref_types_for_widening() {
        let mut w = World::new();
        let ty = w.store.types.alloc(Type::NoType);
        assert_eq!(
            TypeNormalizer::new(&mut w.store).widen_term_ref(ty),
            Err(TypeNormalizeError::NotTermRef { ty })
        );
    }

    #[test]
    fn lookup_normalization_unwraps_flexible_types() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let flexible = w.store.types.alloc(Type::Flexible { underlying: leaf });
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(flexible),
            Ok(leaf)
        );
    }

    #[test]
    fn lookup_normalization_unwraps_annotated_types() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let annotation = w
            .store
            .annotations
            .alloc(dotty_core::types::Annotation::new(leaf, None));
        let annotated = w.store.types.alloc(Type::Annotated {
            underlying: leaf,
            annotation,
        });
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(annotated),
            Ok(leaf)
        );
    }

    #[test]
    fn lookup_normalization_repeats_dealiasing_and_proxy_unwrapping() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (_, alias) = w.alias("A", leaf, SymbolFlags::EMPTY);
        let flexible = w.store.types.alloc(Type::Flexible { underlying: alias });
        let annotation = w
            .store
            .annotations
            .alloc(dotty_core::types::Annotation::new(leaf, None));
        let annotated = w.store.types.alloc(Type::Annotated {
            underlying: flexible,
            annotation,
        });
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(annotated),
            Ok(leaf)
        );
    }

    #[test]
    fn lookup_normalization_preserves_applied_types_without_normalizing_children() {
        let mut w = World::new();
        let symbol = w.symbol(
            "C",
            SymbolKind::Class,
            SymbolFlags::EMPTY,
            SymbolInfo::Missing,
        );
        let tycon = w.type_ref(symbol);
        let arg = w.store.types.alloc(Type::TypeRef {
            prefix: w.prefix,
            target: TypeRefTarget::Symbol(symbol),
        });
        let applied = w.store.types.alloc(Type::Applied {
            tycon,
            args: vec![arg],
        });
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(applied),
            Ok(applied)
        );
        assert_eq!(
            TypeNormalizer::new(&mut w.store).dealias_top(applied),
            Ok(applied)
        );
    }

    #[test]
    fn lookup_normalization_detects_cyclic_proxy_graphs() {
        let mut w = World::new();
        let reserved = w.store.types.reserve();
        w.store.types.fill(
            reserved,
            Type::Flexible {
                underlying: reserved.id(),
            },
        );
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(reserved.id()),
            Err(TypeNormalizeError::NormalizationCycle { ty: reserved.id() })
        );
    }

    #[test]
    fn lookup_normalization_bounds_proxy_depth() {
        let mut w = World::new();
        let mut current = w.store.types.alloc(Type::NoType);
        for _ in 0..=MAX_TYPE_NORMALIZATION_DEPTH {
            current = w.store.types.alloc(Type::Flexible {
                underlying: current,
            });
        }
        assert_eq!(
            TypeNormalizer::new(&mut w.store).normalize_for_lookup(current),
            Err(TypeNormalizeError::TooDeep)
        );
    }
}
