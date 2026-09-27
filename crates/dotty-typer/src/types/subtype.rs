//! Bounded nominal subtyping and conformance over the supported source types.

use std::collections::HashSet;
use std::fmt;

use dotty_core::types::{Type, TypeRefTarget};
use dotty_core::{ScopeId, SymbolId, SymbolKind, TypeId};

use crate::types::{SymbolInfoState, TypeNormalizeError, TypeNormalizer};
use crate::{SourceTyper, TyperError};

/// Maximum parent edges followed by one nominal relation request.
pub const MAX_TYPE_RELATION_DEPTH: usize = 256;

/// A malformed or unsupported semantic type relation.
#[derive(Debug)]
pub enum TypeRelationError {
    /// Alias/proxy normalization failed before nominal rules could be applied.
    Normalization(TypeNormalizeError),
    /// A type shape lies outside the relation supported by this typer slice.
    UnsupportedType { found: TypeId, expected: TypeId },
    /// A class symbol referenced by a nominal type is not in the store.
    UnknownClassSymbol { symbol: SymbolId },
    /// A nominal type reference targets a symbol that is not class-like or a
    /// type parameter.
    NonClassLikeSymbol { symbol: SymbolId, kind: SymbolKind },
    /// The class info needed to inspect parent declarations is not complete.
    ClassInfoUnavailable {
        symbol: SymbolId,
        state: SymbolInfoState,
    },
    /// A completed class points outside the type arena.
    InvalidClassInfoType { symbol: SymbolId, info: TypeId },
    /// A completed class points at something other than `Type::ClassInfo`.
    ClassInfoNotClassInfo { symbol: SymbolId, info: TypeId },
    /// ClassInfo records an identity different from its owning symbol.
    MalformedClassInfoIdentity {
        symbol: SymbolId,
        recorded_class: SymbolId,
    },
    /// ClassInfo points at a declaration scope outside the scope arena.
    InvalidDeclarationScope { symbol: SymbolId, scope: ScopeId },
    /// A source class's parent view could not be instantiated.
    ParentTypeAdaptation { symbol: SymbolId, error: TyperError },
    /// An external applied generic class has no modeled parameter order.
    ExternalGenericInstantiationDeferred { class: SymbolId },
    /// A parent graph revisits a class along one inheritance path.
    InheritanceCycle { symbol: SymbolId },
    /// The nominal inheritance path exceeds [`MAX_TYPE_RELATION_DEPTH`].
    TooDeep,
    /// The graph contains more distinct instantiated parent views than the
    /// relation's bounded worklist permits.
    TooManyParentViews,
}

impl fmt::Display for TypeRelationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TypeRelationError {}

impl SourceTyper<'_> {
    /// Tests the supported nominal subtype relation without completing symbols.
    ///
    /// The relation reads already completed class information. Call
    /// [`SourceTyper::complete_symbol`] explicitly first when source class
    /// completion is desired. `And`, `Or`, methodic, refined, recursive, match,
    /// wildcard, error, and name-designed structural types return
    /// [`TypeRelationError::UnsupportedType`] instead of being treated as
    /// unrelated types.
    pub fn is_subtype(
        &mut self,
        found: TypeId,
        expected: TypeId,
    ) -> Result<bool, TypeRelationError> {
        let mut relation = TypeRelation::new(self);
        relation.is_subtype(found, expected)
    }

    /// Tests value conformance for the supported fragment.
    ///
    /// In this first iteration conformance is exactly [`Self::is_subtype`];
    /// implicit conversions and numeric adaptations are intentionally absent.
    pub fn conforms(&mut self, found: TypeId, expected: TypeId) -> Result<bool, TypeRelationError> {
        self.is_subtype(found, expected)
    }
}

struct TypeRelation<'typer, 'store> {
    typer: &'typer mut SourceTyper<'store>,
    equivalent_pairs: HashSet<(TypeId, TypeId)>,
}

impl<'typer, 'store> TypeRelation<'typer, 'store> {
    fn new(typer: &'typer mut SourceTyper<'store>) -> Self {
        Self {
            typer,
            equivalent_pairs: HashSet::new(),
        }
    }

    fn is_subtype(&mut self, found: TypeId, expected: TypeId) -> Result<bool, TypeRelationError> {
        let found = self.normalize(found)?;
        let expected = self.normalize(expected)?;
        self.validate_supported(
            found,
            expected,
            0,
            true,
            &mut HashSet::new(),
            &mut HashSet::new(),
        )?;
        self.validate_supported(
            expected,
            found,
            0,
            true,
            &mut HashSet::new(),
            &mut HashSet::new(),
        )?;

        if self.equivalent(found, expected, 0)? {
            return Ok(true);
        }
        if matches!(self.type_at(found)?, Type::ByName { .. })
            || matches!(self.type_at(expected)?, Type::ByName { .. })
        {
            return Err(TypeRelationError::UnsupportedType { found, expected });
        }
        if self.is_class(found, self.typer.definitions.nothing_class) {
            return Ok(true);
        }
        if self.is_class(expected, self.typer.definitions.any_class) {
            return Ok(true);
        }

        let found_node = self.type_at(found)?.clone();
        let expected_node = self.type_at(expected)?.clone();
        if let (
            Type::ThisType { class },
            Type::TypeRef {
                target: TypeRefTarget::Symbol(expected_class),
                ..
            },
        ) = (&found_node, &expected_node)
        {
            return Ok(class == expected_class);
        }

        // The nominal graph walk is added in the inheritance increment. Until
        // then unrelated, supported classes are a sound negative answer.
        Ok(false)
    }

    fn normalize(&self, ty: TypeId) -> Result<TypeId, TypeRelationError> {
        TypeNormalizer::new(self.typer.store)
            .normalize_for_lookup(ty)
            .map_err(TypeRelationError::Normalization)
    }

    fn type_at(&self, ty: TypeId) -> Result<&Type, TypeRelationError> {
        self.typer.store.types.try_get(ty).ok_or_else(|| {
            if self.typer.store.types.contains(ty) {
                TypeRelationError::Normalization(TypeNormalizeError::UnfilledType { ty })
            } else {
                TypeRelationError::Normalization(TypeNormalizeError::InvalidType { ty })
            }
        })
    }

    fn is_class(&self, ty: TypeId, symbol: SymbolId) -> bool {
        matches!(
            self.typer.store.types.try_get(ty),
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(found),
                ..
            }) if *found == symbol
        )
    }

    fn validate_supported(
        &self,
        found: TypeId,
        expected: TypeId,
        depth: usize,
        value_position: bool,
        active: &mut HashSet<(TypeId, bool)>,
        completed: &mut HashSet<(TypeId, bool)>,
    ) -> Result<(), TypeRelationError> {
        if depth >= MAX_TYPE_RELATION_DEPTH {
            return Err(TypeRelationError::TooDeep);
        }
        let found = self.normalize(found)?;
        let state = (found, value_position);
        if completed.contains(&state) {
            return Ok(());
        }
        if !active.insert(state) {
            return Err(TypeRelationError::UnsupportedType { found, expected });
        }
        let node = self.type_at(found)?.clone();
        let mut children = Vec::new();
        match node {
            Type::NoPrefix if !value_position => {}
            Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(symbol),
            } => {
                if !self.typer.store.symbols.contains(symbol) {
                    return Err(TypeRelationError::UnknownClassSymbol { symbol });
                }
                let declaration = self.typer.store.symbols.get(symbol);
                if value_position
                    && !matches!(
                        declaration.kind,
                        SymbolKind::Class
                            | SymbolKind::Trait
                            | SymbolKind::ModuleClass
                            | SymbolKind::TypeParameter
                    )
                {
                    return Err(TypeRelationError::NonClassLikeSymbol {
                        symbol,
                        kind: declaration.kind,
                    });
                }
                children.push((prefix, false));
            }
            Type::TypeRef {
                target: TypeRefTarget::Name(_),
                ..
            } => return Err(TypeRelationError::UnsupportedType { found, expected }),
            Type::ThisType { class } => {
                self.ensure_class_symbol(class)?;
            }
            Type::Applied { tycon, args } => {
                children.push((tycon, true));
                children.extend(args.into_iter().map(|argument| (argument, true)));
            }
            Type::ByName { result } => children.push((result, true)),
            Type::ParamRef { binder, index } => {
                let binder_type = self.type_at(binder)?;
                let arity = match binder_type {
                    Type::Method(method) => method.params.len(),
                    Type::Poly(poly) => poly.params.len(),
                    Type::TypeLambda(lambda) => lambda.params.len(),
                    _ => return Err(TypeRelationError::UnsupportedType { found, expected }),
                };
                if index as usize >= arity {
                    return Err(TypeRelationError::UnsupportedType { found, expected });
                }
            }
            _ => {
                return Err(TypeRelationError::UnsupportedType { found, expected });
            }
        }
        for (child, child_is_value) in children {
            self.validate_supported(
                child,
                expected,
                depth + 1,
                child_is_value,
                active,
                completed,
            )?;
        }
        active.remove(&state);
        completed.insert(state);
        Ok(())
    }

    fn ensure_class_symbol(&self, symbol: SymbolId) -> Result<(), TypeRelationError> {
        if !self.typer.store.symbols.contains(symbol) {
            return Err(TypeRelationError::UnknownClassSymbol { symbol });
        }
        let declaration = self.typer.store.symbols.get(symbol);
        if matches!(
            declaration.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            Ok(())
        } else {
            Err(TypeRelationError::NonClassLikeSymbol {
                symbol,
                kind: declaration.kind,
            })
        }
    }

    fn equivalent(
        &mut self,
        left: TypeId,
        right: TypeId,
        depth: usize,
    ) -> Result<bool, TypeRelationError> {
        if depth >= MAX_TYPE_RELATION_DEPTH {
            return Err(TypeRelationError::TooDeep);
        }
        let left = self.normalize(left)?;
        let right = self.normalize(right)?;
        if left == right {
            return Ok(true);
        }
        if !self.equivalent_pairs.insert((left, right)) {
            return Ok(true);
        }
        let left_node = self.type_at(left)?.clone();
        let right_node = self.type_at(right)?.clone();
        match (left_node, right_node) {
            (Type::NoPrefix, Type::NoPrefix) => Ok(true),
            (
                Type::TypeRef {
                    prefix: left_prefix,
                    target: TypeRefTarget::Symbol(left_symbol),
                },
                Type::TypeRef {
                    prefix: right_prefix,
                    target: TypeRefTarget::Symbol(right_symbol),
                },
            ) => {
                if left_symbol != right_symbol {
                    return Ok(false);
                }
                self.equivalent(left_prefix, right_prefix, depth + 1)
            }
            (Type::ThisType { class: left }, Type::ThisType { class: right }) => Ok(left == right),
            (
                Type::Applied {
                    tycon: left_tycon,
                    args: left_args,
                },
                Type::Applied {
                    tycon: right_tycon,
                    args: right_args,
                },
            ) => {
                if !self.equivalent(left_tycon, right_tycon, depth + 1)? {
                    return Ok(false);
                }
                if left_args.len() != right_args.len() {
                    return Err(TypeRelationError::UnsupportedType {
                        found: left,
                        expected: right,
                    });
                }
                for (left, right) in left_args.into_iter().zip(right_args) {
                    if !self.equivalent(left, right, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (
                Type::ParamRef {
                    binder: left_binder,
                    index: left_index,
                },
                Type::ParamRef {
                    binder: right_binder,
                    index: right_index,
                },
            ) => Ok(left_binder == right_binder && left_index == right_index),
            (Type::ByName { result: left }, Type::ByName { result: right }) => {
                self.equivalent(left, right, depth + 1)
            }
            _ => Ok(false),
        }
    }
}
