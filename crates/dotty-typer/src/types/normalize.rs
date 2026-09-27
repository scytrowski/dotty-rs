//! Bounded, read-only operations on the outermost semantic type.
//!
//! These helpers return existing `TypeId`s and never complete symbols or
//! allocate semantic values. They intentionally do not perform substitution,
//! subtyping, member lookup, or recursive normalization of child types.

use std::fmt;

use dotty_core::types::{Type, TypeRefTarget};
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
    /// A symbol reference points outside this semantic store.
    UnknownSymbol { symbol: SymbolId },
    /// A type alias is opaque at this stage; its representation is hidden.
    OpaqueAlias { symbol: SymbolId },
    /// A type alias needs type-parameter substitution before it can be opened.
    AliasRequiresSubstitution { symbol: SymbolId },
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

/// Read-only, bounded semantic type normalization over an existing store.
pub struct TypeNormalizer<'a> {
    store: &'a SemanticStore,
}

impl<'a> TypeNormalizer<'a> {
    /// Creates a normalizer over the supplied semantic store.
    pub const fn new(store: &'a SemanticStore) -> Self {
        Self { store }
    }

    /// Opens a chain of simple, non-opaque type aliases at the top level.
    ///
    /// Name-target references, non-alias symbols, and non-alias type forms are
    /// returned unchanged. Alias declarations must already have complete
    /// `AliasingBounds` info. Generic aliases are rejected because opening one
    /// without substitution would return a semantically incorrect type.
    pub fn dealias_top(&self, ty: TypeId) -> Result<TypeId, TypeNormalizeError> {
        self.dealias_top_with_budget(ty, MAX_TYPE_NORMALIZATION_DEPTH)
            .map(|(ty, _)| ty)
    }

    fn dealias_top_with_budget(
        &self,
        ty: TypeId,
        budget: usize,
    ) -> Result<(TypeId, usize), TypeNormalizeError> {
        let mut current = ty;
        let mut visited = Vec::new();
        let mut links = 0;

        for _ in 0..=budget {
            if !self.store.types.is_filled(current) {
                return Err(TypeNormalizeError::UnfilledType { ty: current });
            }
            let (symbol, next) = match self.store.types.get(current) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } => {
                    let Some(next) = self.alias_target(*symbol)? else {
                        return Ok((current, links));
                    };
                    (*symbol, next)
                }
                Type::Applied { tycon, .. } => {
                    if let Some(symbol) = self.alias_symbol(*tycon)? {
                        if !self.store.symbols.contains(symbol) {
                            return Err(TypeNormalizeError::UnknownSymbol { symbol });
                        }
                        if self
                            .store
                            .symbols
                            .get(symbol)
                            .flags
                            .contains(SymbolFlags::OPAQUE)
                        {
                            return Err(TypeNormalizeError::OpaqueAlias { symbol });
                        }
                        return Err(TypeNormalizeError::AliasRequiresSubstitution { symbol });
                    }
                    return Ok((current, links));
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
            links += 1;
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
        if !self.store.types.is_filled(ty) {
            return Err(TypeNormalizeError::UnfilledType { ty });
        }
        let Type::TermRef { target, .. } = self.store.types.get(ty) else {
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
                if !self.store.types.is_filled(info) {
                    return Err(TypeNormalizeError::UnfilledType { ty: info });
                }
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
    /// The operation preserves `Applied` and all other type forms, and never
    /// descends into their children. Alias links and wrappers share one depth
    /// budget, and repeated outer nodes are reported as malformed cycles.
    pub fn normalize_for_lookup(&self, ty: TypeId) -> Result<TypeId, TypeNormalizeError> {
        let mut current = ty;
        let mut visited = Vec::new();
        let mut depth = 0;

        loop {
            if visited.contains(&current) {
                return Err(TypeNormalizeError::NormalizationCycle { ty: current });
            }
            visited.push(current);
            if !self.store.types.is_filled(current) {
                return Err(TypeNormalizeError::UnfilledType { ty: current });
            }
            // Applied aliases require substitution. Lookup normalization is
            // intentionally shallow, so it preserves the entire application.
            if matches!(self.store.types.get(current), Type::Applied { .. }) {
                return Ok(current);
            }
            let (dealiased, links) = self.dealias_top_with_budget(
                current,
                MAX_TYPE_NORMALIZATION_DEPTH.saturating_sub(depth),
            )?;
            depth += links;
            if dealiased != current {
                current = dealiased;
                continue;
            }
            let underlying = match self.store.types.get(current) {
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
        if !self.store.types.is_filled(ty) {
            return Err(TypeNormalizeError::UnfilledType { ty });
        }
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } = self.store.types.get(ty)
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
        if !self.store.types.is_filled(info) {
            return Err(TypeNormalizeError::UnfilledType { ty: info });
        }
        match self.store.types.get(info) {
            Type::AliasingBounds { alias } => Ok(Some(*alias)),
            _ => Err(TypeNormalizeError::AliasInfoNotAliasingBounds { symbol, info }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::types::{TermRefTarget, Type};
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
    }

    #[test]
    fn dealiases_a_simple_complete_alias_without_allocating() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (_, reference) = w.alias("A", leaf, SymbolFlags::EMPTY);
        let marker = w.store.types.alloc(Type::NoType);
        assert_eq!(
            TypeNormalizer::new(&w.store).dealias_top(reference),
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
        assert_eq!(TypeNormalizer::new(&w.store).dealias_top(outer), Ok(leaf));
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
        let normalizer = TypeNormalizer::new(&w.store);
        assert_eq!(normalizer.dealias_top(class_ref), Ok(class_ref));
        assert_eq!(normalizer.dealias_top(name_ref), Ok(name_ref));
    }

    #[test]
    fn rejects_opaque_aliases_without_opening_them() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (symbol, reference) = w.alias("Opaque", leaf, SymbolFlags::OPAQUE);
        assert_eq!(
            TypeNormalizer::new(&w.store).dealias_top(reference),
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
                TypeNormalizer::new(&w.store).dealias_top(reference),
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
            TypeNormalizer::new(&w.store).dealias_top(reference),
            Err(TypeNormalizeError::AliasInfoNotAliasingBounds { symbol, info })
        );
    }

    #[test]
    fn rejects_applied_generic_aliases_without_substitution() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (symbol, reference) = w.alias("A", leaf, SymbolFlags::EMPTY);
        let argument = w.store.types.alloc(Type::NoType);
        let applied = w.store.types.alloc(Type::Applied {
            tycon: reference,
            args: vec![argument],
        });
        assert_eq!(
            TypeNormalizer::new(&w.store).dealias_top(applied),
            Err(TypeNormalizeError::AliasRequiresSubstitution { symbol })
        );
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
            TypeNormalizer::new(&w.store).dealias_top(reference),
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
            TypeNormalizer::new(&w.store).dealias_top(outer),
            Err(TypeNormalizeError::TooDeep)
        );
    }

    #[test]
    fn reports_an_unfilled_type_slot_instead_of_reading_it() {
        let mut w = World::new();
        let reserved = w.store.types.reserve();
        assert_eq!(
            TypeNormalizer::new(&w.store).dealias_top(reserved.id()),
            Err(TypeNormalizeError::UnfilledType { ty: reserved.id() })
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
            TypeNormalizer::new(&w.store).dealias_top(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
                TypeNormalizer::new(&w.store).widen_term_ref(reference),
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
            TypeNormalizer::new(&w.store).widen_term_ref(ty),
            Err(TypeNormalizeError::NotTermRef { ty })
        );
    }

    #[test]
    fn lookup_normalization_unwraps_flexible_types() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let flexible = w.store.types.alloc(Type::Flexible { underlying: leaf });
        assert_eq!(
            TypeNormalizer::new(&w.store).normalize_for_lookup(flexible),
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
            TypeNormalizer::new(&w.store).normalize_for_lookup(annotated),
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
            TypeNormalizer::new(&w.store).normalize_for_lookup(annotated),
            Ok(leaf)
        );
    }

    #[test]
    fn lookup_normalization_preserves_applied_types_without_normalizing_children() {
        let mut w = World::new();
        let leaf = w.store.types.alloc(Type::NoType);
        let (symbol, alias) = w.alias("A", leaf, SymbolFlags::EMPTY);
        let arg = w.store.types.alloc(Type::TypeRef {
            prefix: w.prefix,
            target: TypeRefTarget::Symbol(symbol),
        });
        let applied = w.store.types.alloc(Type::Applied {
            tycon: alias,
            args: vec![arg],
        });
        assert_eq!(
            TypeNormalizer::new(&w.store).normalize_for_lookup(applied),
            Ok(applied)
        );
        assert_eq!(
            TypeNormalizer::new(&w.store).dealias_top(applied),
            Err(TypeNormalizeError::AliasRequiresSubstitution { symbol })
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
            TypeNormalizer::new(&w.store).normalize_for_lookup(reserved.id()),
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
            TypeNormalizer::new(&w.store).normalize_for_lookup(current),
            Err(TypeNormalizeError::TooDeep)
        );
    }
}
