//! Bounded nominal subtyping and conformance over the supported source types.

use std::collections::{HashMap, HashSet};
use std::fmt;

use dotty_core::types::{ClassInfo, Type, TypeRefTarget};
use dotty_core::{ScopeId, SymbolId, SymbolInfo, SymbolKind, TypeId};

use crate::types::{SymbolInfoState, TypeNormalizeError, TypeNormalizer};
use crate::{SourceTyper, TyperError};

/// Maximum parent edges followed by one nominal relation request.
pub const MAX_TYPE_RELATION_DEPTH: usize = 256;
/// Maximum instantiated parent views inspected by one relation.
pub const MAX_TYPE_RELATION_VIEWS: usize = 4096;

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
    /// A parent entry does not designate a class-like type.
    MalformedParentType { ty: TypeId },
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
    equivalent_pairs: HashMap<(TypeId, TypeId), bool>,
    active_equivalent_pairs: HashSet<(TypeId, TypeId)>,
}

impl<'typer, 'store> TypeRelation<'typer, 'store> {
    fn new(typer: &'typer mut SourceTyper<'store>) -> Self {
        Self {
            typer,
            equivalent_pairs: HashMap::new(),
            active_equivalent_pairs: HashSet::new(),
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

        let found_view = self.class_view(found)?;
        let expected_view = self.class_view(expected)?;
        self.reject_unmodeled_external_generic(&found_view)?;
        self.reject_unmodeled_external_generic(&expected_view)?;
        self.check_view_arity(&found_view, &expected_view, found, expected)?;
        if found_view.class == expected_view.class {
            return self.equivalent_class_views(&found_view, &expected_view, 0);
        }
        self.inherited_subtype(found_view, expected_view, expected)
    }

    fn reject_unmodeled_external_generic(&self, view: &ClassView) -> Result<(), TypeRelationError> {
        if view.applied && !view.args.is_empty() && !self.typer.is_current_source_symbol(view.class)
        {
            return Err(TypeRelationError::ExternalGenericInstantiationDeferred {
                class: view.class,
            });
        }
        Ok(())
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

    fn class_view(&self, ty: TypeId) -> Result<ClassView, TypeRelationError> {
        let ty = self.normalize(ty)?;
        let (tycon, args, applied) = match self.type_at(ty)? {
            Type::Applied { tycon, args } => (*tycon, args.clone(), true),
            _ => (ty, Vec::new(), false),
        };
        let tycon = self.normalize(tycon)?;
        let class = match self.type_at(tycon)? {
            Type::ThisType { class } => *class,
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            _ => {
                return Err(TypeRelationError::UnsupportedType {
                    found: ty,
                    expected: ty,
                });
            }
        };
        self.ensure_class_symbol(class)?;
        Ok(ClassView {
            ty,
            class,
            args,
            applied,
        })
    }

    fn check_view_arity(
        &self,
        left: &ClassView,
        right: &ClassView,
        found: TypeId,
        expected: TypeId,
    ) -> Result<(), TypeRelationError> {
        if left.class == right.class && left.applied != right.applied {
            return Err(TypeRelationError::UnsupportedType { found, expected });
        }
        if left.class == right.class && left.applied && left.args.len() != right.args.len() {
            return Err(TypeRelationError::UnsupportedType { found, expected });
        }
        Ok(())
    }

    fn inherited_subtype(
        &mut self,
        found: ClassView,
        expected: ClassView,
        expected_type: TypeId,
    ) -> Result<bool, TypeRelationError> {
        let found_class = found.class;
        let mut pending_views = vec![PendingView {
            view: found,
            depth: 0,
            path: vec![found_class],
        }];
        let mut inspected_views = 1;
        let mut matched_expected = false;

        while let Some(pending) = pending_views.pop() {
            let info = self.class_info(pending.view.class)?;
            if info.parents.is_empty() {
                continue;
            }
            if pending.depth >= MAX_TYPE_RELATION_DEPTH {
                return Err(TypeRelationError::TooDeep);
            }
            let mut children = Vec::new();
            for parent in info.parents {
                let parent = self
                    .typer
                    .adapt_parent_view(pending.view.class, pending.view.ty, parent)
                    .map_err(|error| match error {
                        TyperError::ExternalGenericInstantiationDeferred { class } => {
                            TypeRelationError::ExternalGenericInstantiationDeferred { class }
                        }
                        other => TypeRelationError::ParentTypeAdaptation {
                            symbol: pending.view.class,
                            error: other,
                        },
                    })?;
                let parent_view = self.class_view(parent).map_err(|error| match error {
                    TypeRelationError::UnsupportedType { .. } => {
                        TypeRelationError::MalformedParentType { ty: parent }
                    }
                    other => other,
                })?;

                if pending.path.contains(&parent_view.class) {
                    return Err(TypeRelationError::InheritanceCycle {
                        symbol: parent_view.class,
                    });
                }

                if parent_view.class == expected.class {
                    self.check_view_arity(&parent_view, &expected, parent, expected_type)?;
                    if self.equivalent_class_views(&parent_view, &expected, 0)? {
                        matched_expected = true;
                    }
                }

                if matches!(
                    parent_view.class,
                    class if class == self.typer.definitions.object_class
                        || class == self.typer.definitions.any_class
                        || class == self.typer.definitions.nothing_class
                ) {
                    continue;
                }
                if pending.depth + 1 > MAX_TYPE_RELATION_DEPTH {
                    return Err(TypeRelationError::TooDeep);
                }

                if inspected_views >= MAX_TYPE_RELATION_VIEWS {
                    return Err(TypeRelationError::TooManyParentViews);
                }
                inspected_views += 1;
                let mut path = pending.path.clone();
                path.push(parent_view.class);
                children.push(PendingView {
                    view: parent_view,
                    depth: pending.depth + 1,
                    path,
                });
            }
            pending_views.extend(children.into_iter().rev());
        }

        Ok(matched_expected)
    }

    fn equivalent_class_views(
        &mut self,
        left: &ClassView,
        right: &ClassView,
        depth: usize,
    ) -> Result<bool, TypeRelationError> {
        if left.class != right.class || left.applied != right.applied {
            return Ok(false);
        }
        if left.args.len() != right.args.len() {
            return Err(TypeRelationError::UnsupportedType {
                found: left.ty,
                expected: right.ty,
            });
        }
        for (left, right) in left.args.iter().zip(&right.args) {
            if !self.equivalent(*left, *right, depth + 1)? {
                return Ok(false);
            }
        }

        let left_constructor = self.type_constructor(left.ty)?;
        let right_constructor = self.type_constructor(right.ty)?;
        match (
            self.type_at(left_constructor)?,
            self.type_at(right_constructor)?,
        ) {
            (Type::ThisType { class: left }, Type::ThisType { class: right }) => {
                return Ok(left == right);
            }
            (Type::TypeRef { .. }, Type::TypeRef { .. }) => {}
            _ => return Ok(false),
        }

        self.equivalent(left_constructor, right_constructor, depth + 1)
    }

    fn type_constructor(&self, ty: TypeId) -> Result<TypeId, TypeRelationError> {
        match self.type_at(ty)? {
            Type::Applied { tycon, .. } => Ok(*tycon),
            _ => Ok(ty),
        }
    }

    fn class_info(&self, symbol: SymbolId) -> Result<ClassInfo, TypeRelationError> {
        if !self.typer.store.symbols.contains(symbol) {
            return Err(TypeRelationError::UnknownClassSymbol { symbol });
        }
        let info_type = match *self.typer.store.symbols.info(symbol) {
            SymbolInfo::Complete(info) => info,
            SymbolInfo::Missing => {
                return Err(TypeRelationError::ClassInfoUnavailable {
                    symbol,
                    state: SymbolInfoState::Missing,
                });
            }
            SymbolInfo::Deferred(_) => {
                return Err(TypeRelationError::ClassInfoUnavailable {
                    symbol,
                    state: SymbolInfoState::Deferred,
                });
            }
            SymbolInfo::Error => {
                return Err(TypeRelationError::ClassInfoUnavailable {
                    symbol,
                    state: SymbolInfoState::Error,
                });
            }
        };
        let Some(info_node) = self.typer.store.types.try_get(info_type) else {
            return Err(TypeRelationError::InvalidClassInfoType {
                symbol,
                info: info_type,
            });
        };
        let Type::ClassInfo(info) = info_node else {
            return Err(TypeRelationError::ClassInfoNotClassInfo {
                symbol,
                info: info_type,
            });
        };
        if info.class != symbol {
            return Err(TypeRelationError::MalformedClassInfoIdentity {
                symbol,
                recorded_class: info.class,
            });
        }
        if !self.typer.store.scopes.contains(info.declarations) {
            return Err(TypeRelationError::InvalidDeclarationScope {
                symbol,
                scope: info.declarations,
            });
        }
        Ok(info.clone())
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
        let pair = (left, right);
        if let Some(equivalent) = self.equivalent_pairs.get(&pair) {
            return Ok(*equivalent);
        }
        if !self.active_equivalent_pairs.insert(pair) {
            return Ok(true);
        }
        let left_node = self.type_at(left)?.clone();
        let right_node = self.type_at(right)?.clone();
        let result: Result<bool, TypeRelationError> = (|| match (left_node, right_node) {
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
        })();
        self.active_equivalent_pairs.remove(&pair);
        if let Ok(equivalent) = result {
            self.equivalent_pairs.insert(pair, equivalent);
        }
        result
    }
}

#[derive(Clone)]
struct ClassView {
    ty: TypeId,
    class: SymbolId,
    args: Vec<TypeId>,
    applied: bool,
}

struct PendingView {
    view: ClassView,
    depth: usize,
    path: Vec<SymbolId>,
}
