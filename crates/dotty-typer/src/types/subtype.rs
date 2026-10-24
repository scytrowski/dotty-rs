//! Bounded subtyping and conformance over supported nominal and constant types.

use std::collections::{HashMap, HashSet};
use std::fmt;

use dotty_core::Namespace;
use dotty_core::types::{ClassInfo, Type, TypeRefTarget};
use dotty_core::{Constant, ScopeId, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TypeId};

use crate::types::{SymbolInfoState, TypeNormalizeError, TypeNormalizer};
use crate::{SourceTyper, TyperError};

/// Maximum parent edges followed by one nominal relation request.
pub const MAX_TYPE_RELATION_DEPTH: usize = 256;
/// Maximum instantiated parent views inspected by one relation.
pub const MAX_TYPE_RELATION_VIEWS: usize = 4096;
/// Maximum union comparisons expanded by one subtype relation.
pub const MAX_UNION_RELATION_COMPARISONS: usize = 4096;

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
    /// A current source class's completed info does not use its named scope.
    MalformedClassInfoDeclarations {
        symbol: SymbolId,
        declarations: ScopeId,
        expected: Option<ScopeId>,
    },
    /// A class declaration scope contains a symbol outside the symbol arena.
    InvalidClassDeclarationSymbol {
        symbol: SymbolId,
        declaration: SymbolId,
    },
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
    /// A union relation expanded more comparisons than its work limit permits.
    TooManyUnionRelations,
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
    /// completion is desired. Supported `Type::Constant` values compare by
    /// exact payload and conform to their modeled builtin type. String
    /// constants support exact equality but have no modeled underlying type.
    /// `Or` uses the bounded union rules documented on [`Self::conforms`].
    /// Intersection, methodic, refined, recursive, match, wildcard, error, and
    /// name-designed structural types return
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
    /// Conformance uses [`Self::is_subtype`], including bounded rules for
    /// `Or` types created by expression joins. Implicit conversions and
    /// numeric adaptations are intentionally absent.
    pub fn conforms(&mut self, found: TypeId, expected: TypeId) -> Result<bool, TypeRelationError> {
        self.is_subtype(found, expected)
    }
}

struct TypeRelation<'typer, 'store> {
    typer: &'typer mut SourceTyper<'store>,
    equivalent_pairs: HashMap<(TypeId, TypeId), bool>,
    active_equivalent_pairs: HashSet<(TypeId, TypeId)>,
    union_comparisons: usize,
}

impl<'typer, 'store> TypeRelation<'typer, 'store> {
    fn new(typer: &'typer mut SourceTyper<'store>) -> Self {
        Self {
            typer,
            equivalent_pairs: HashMap::new(),
            active_equivalent_pairs: HashSet::new(),
            union_comparisons: 0,
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
        let found_node = self.type_at(found)?.clone();
        let expected_node = self.type_at(expected)?.clone();
        if let Type::Or { left, right } = found_node {
            self.bump_union_comparison()?;
            let left_result = self.is_subtype(left, expected);
            let right_result = self.is_subtype(right, expected);
            return match (left_result, right_result) {
                (Ok(false), _) | (_, Ok(false)) => Ok(false),
                (Ok(true), Ok(true)) => Ok(true),
                (Err(error), _) | (_, Err(error)) => Err(error),
            };
        }
        if let Type::Or { left, right } = expected_node {
            self.bump_union_comparison()?;
            let left_result = self.is_subtype(found, left);
            let right_result = self.is_subtype(found, right);
            return match (left_result, right_result) {
                (Ok(true), _) | (_, Ok(true)) => Ok(true),
                (Ok(false), Ok(false)) => Ok(false),
                (Err(error), _) | (_, Err(error)) => Err(error),
            };
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
        if self.is_class(expected, self.typer.definitions.null_class) {
            return Ok(false);
        }
        if self.is_class(found, self.typer.definitions.null_class) {
            return self.null_conforms_to_reference(&expected_node);
        }
        if self.is_builtin_class(found) && self.is_builtin_class(expected) {
            return Ok(false);
        }

        let found_array_element = self.array_element(found)?;
        let expected_array_element = self.array_element(expected)?;
        if found_array_element.is_some() || expected_array_element.is_some() {
            return match (found_array_element, expected_array_element) {
                (Some(found), Some(expected)) => self.equivalent(found, expected, 0),
                _ => Ok(false),
            };
        }

        match (&found_node, &expected_node) {
            (Type::Constant(found_constant), Type::Constant(expected_constant)) => {
                return Ok(found_constant == expected_constant);
            }
            (_, Type::Constant(_)) => return Ok(false),
            (Type::Constant(constant), _) => {
                let Some(underlying) = self.typer.constant_underlying_type(constant) else {
                    return Err(TypeRelationError::UnsupportedType { found, expected });
                };
                return self.is_subtype(underlying, expected);
            }
            _ => {}
        }

        if let (
            Type::ThisType { class },
            Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(expected_class),
            },
        ) = (&found_node, &expected_node)
        {
            return Ok(class == expected_class
                && self.this_type_reference_prefix_matches(*class, *prefix)?);
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

    fn bump_union_comparison(&mut self) -> Result<(), TypeRelationError> {
        if self.union_comparisons >= MAX_UNION_RELATION_COMPARISONS {
            return Err(TypeRelationError::TooManyUnionRelations);
        }
        self.union_comparisons += 1;
        Ok(())
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

    fn is_builtin_class(&self, ty: TypeId) -> bool {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) = self.typer.store.types.try_get(ty)
        else {
            return false;
        };
        self.typer.store.symbols.contains(*symbol)
            && self.typer.store.symbols.get(*symbol).kind == SymbolKind::Class
            && self.typer.store.symbols.get(*symbol).origin == SymbolOrigin::Builtin
    }

    fn null_conforms_to_reference(&self, expected: &Type) -> Result<bool, TypeRelationError> {
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        } = expected
        else {
            return Ok(false);
        };
        if *class == self.typer.definitions.object_class {
            return Ok(*prefix == self.typer.definitions.no_prefix);
        }
        if !self.typer.is_current_source_symbol(*class)
            || self.typer.store.symbols.get(*class).kind != SymbolKind::Class
        {
            return Ok(false);
        }
        if !self.is_canonical_top_level_class_prefix(*class, *prefix)? {
            return Ok(false);
        }

        let info = self.class_info(*class)?;
        let expected_declarations = self.typer.index.scope_of(*class);
        if expected_declarations != Some(info.declarations) {
            return Err(TypeRelationError::MalformedClassInfoDeclarations {
                symbol: *class,
                declarations: info.declarations,
                expected: expected_declarations,
            });
        }
        let declarations = self.typer.store.scopes.get(info.declarations);
        for declaration in declarations.entered_symbols() {
            if !self.typer.store.symbols.contains(declaration) {
                return Err(TypeRelationError::InvalidClassDeclarationSymbol {
                    symbol: *class,
                    declaration,
                });
            }
            let declaration = self.typer.store.symbols.get(declaration);
            if declaration.kind == SymbolKind::TypeParameter {
                // The current increment does not model raw generic class
                // types. Conservatively keep `Null` conformance inside the
                // explicitly supported non-generic class slice.
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn this_type_reference_prefix_matches(
        &self,
        class: SymbolId,
        prefix: TypeId,
    ) -> Result<bool, TypeRelationError> {
        let declaration = self.typer.store.symbols.get(class);
        if let Some(owner) = declaration.owner.filter(|owner| {
            self.typer.store.symbols.contains(*owner)
                && matches!(
                    self.typer.store.symbols.get(*owner).kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                )
        }) {
            // `ThisType` carries no enclosing-instance path. Accept the
            // canonical owner `this` prefix, but reject a different outer
            // instance because it may select a distinct path-dependent class.
            return Ok(matches!(
                self.type_at(prefix)?,
                Type::ThisType { class: prefix_owner } if *prefix_owner == owner
            ));
        }

        if matches!(self.type_at(prefix)?, Type::NoPrefix) {
            return Ok(true);
        }
        let Some(owner) = declaration.owner else {
            return Ok(false);
        };
        if !self.typer.store.symbols.contains(owner)
            || self.typer.store.symbols.get(owner).kind != SymbolKind::Package
        {
            return Ok(false);
        }
        Ok(matches!(
            self.type_at(prefix)?,
            Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(package),
            } if *package == owner && self.is_canonical_package_prefix(owner, *package_prefix, 0)?
        ))
    }

    fn is_canonical_package_prefix(
        &self,
        package: SymbolId,
        prefix: TypeId,
        depth: usize,
    ) -> Result<bool, TypeRelationError> {
        if depth >= MAX_TYPE_RELATION_DEPTH {
            return Err(TypeRelationError::TooDeep);
        }
        if !self.typer.store.symbols.contains(package)
            || self.typer.store.symbols.get(package).kind != SymbolKind::Package
        {
            return Ok(false);
        }
        let declaration = self.typer.store.symbols.get(package);
        if matches!(self.type_at(prefix)?, Type::NoPrefix) {
            return Ok(true);
        }
        let Some(owner) = declaration.owner else {
            return Ok(false);
        };
        if !self.typer.store.symbols.contains(owner)
            || self.typer.store.symbols.get(owner).kind != SymbolKind::Package
        {
            return Ok(false);
        }
        match self.type_at(prefix)? {
            Type::TypeRef {
                prefix: parent_prefix,
                target: TypeRefTarget::Symbol(parent_package),
            } if *parent_package == owner => {
                self.is_canonical_package_prefix(owner, *parent_prefix, depth + 1)
            }
            _ => Ok(false),
        }
    }

    fn is_canonical_top_level_class_prefix(
        &self,
        class: SymbolId,
        prefix: TypeId,
    ) -> Result<bool, TypeRelationError> {
        let Some(package) = self.typer.store.symbols.get(class).owner.filter(|owner| {
            self.typer.store.symbols.contains(*owner)
                && self.typer.store.symbols.get(*owner).kind == SymbolKind::Package
        }) else {
            return Ok(false);
        };
        if matches!(self.type_at(prefix)?, Type::NoPrefix) {
            return Ok(true);
        }
        match self.type_at(prefix)? {
            Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(found_package),
            } if *found_package == package => {
                self.is_canonical_package_prefix(package, *package_prefix, 0)
            }
            _ => Ok(false),
        }
    }

    fn normalize(&mut self, ty: TypeId) -> Result<TypeId, TypeRelationError> {
        TypeNormalizer::new(self.typer.store)
            .normalize_for_relation(ty)
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

    /// Returns the element type for the JVM-array representation and the
    /// canonical Scala `Array[T]` type. Scala's `Array` is invariant and maps
    /// to JVM arrays, so the two representations conform only when their
    /// element types are equivalent.
    fn array_element(&mut self, ty: TypeId) -> Result<Option<TypeId>, TypeRelationError> {
        let ty = self.normalize(ty)?;
        match self.type_at(ty)?.clone() {
            Type::JavaArray { element } => Ok(Some(element)),
            Type::Applied { tycon, args } if args.len() == 1 => {
                let tycon = self.normalize(tycon)?;
                let Type::TypeRef {
                    target: TypeRefTarget::Symbol(class),
                    ..
                } = self.type_at(tycon)?
                else {
                    return Ok(None);
                };
                if !self.typer.store.symbols.contains(*class) {
                    return Err(TypeRelationError::UnknownClassSymbol { symbol: *class });
                }
                Ok(self.is_scala_array_class(*class).then_some(args[0]))
            }
            _ => Ok(None),
        }
    }

    fn is_scala_array_class(&self, class: SymbolId) -> bool {
        let symbols = &self.typer.store.symbols;
        if !symbols.contains(class) {
            return false;
        }
        let declaration = symbols.get(class);
        if declaration.kind != SymbolKind::Class
            || declaration.name.namespace() != Namespace::Type
            || self.typer.store.names.resolve(declaration.name.text()) != "Array"
        {
            return false;
        }
        let Some(package_id) = declaration.owner else {
            return false;
        };
        if !symbols.contains(package_id) {
            return false;
        }
        let package = symbols.get(package_id);
        if package.kind != SymbolKind::Package
            || package.name.namespace() != Namespace::Term
            || self.typer.store.names.resolve(package.name.text()) != "scala"
        {
            return false;
        }
        let Some(root_id) = package.owner else {
            return false;
        };
        if !symbols.contains(root_id) {
            return false;
        }
        let root = symbols.get(root_id);
        root.kind == SymbolKind::Package
            && root.name.namespace() == Namespace::Term
            && self.typer.store.names.resolve(root.name.text()).is_empty()
            && root.owner.is_none()
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

    fn class_view(&mut self, ty: TypeId) -> Result<ClassView, TypeRelationError> {
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
        &mut self,
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
            Type::JavaArray { element } => children.push((element, true)),
            Type::Constant(
                Constant::Unit
                | Constant::Boolean(_)
                | Constant::Byte(_)
                | Constant::Short(_)
                | Constant::Char(_)
                | Constant::Int(_)
                | Constant::Long(_)
                | Constant::FloatBits(_)
                | Constant::DoubleBits(_)
                | Constant::String(_),
            ) => {}
            Type::Or { left, right } => {
                children.push((left, true));
                children.push((right, true));
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
                if self.is_canonical_top_level_class_prefix(left_symbol, left_prefix)?
                    && self.is_canonical_top_level_class_prefix(left_symbol, right_prefix)?
                {
                    return Ok(true);
                }
                self.equivalent(left_prefix, right_prefix, depth + 1)
            }
            (Type::ThisType { class: left }, Type::ThisType { class: right }) => Ok(left == right),
            (Type::Constant(left), Type::Constant(right)) => Ok(left == right),
            (Type::JavaArray { element: left }, Type::JavaArray { element: right }) => {
                self.equivalent(left, right, depth + 1)
            }
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
            _ => {
                let left_array = self.array_element(left)?;
                let right_array = self.array_element(right)?;
                match (left_array, right_array) {
                    (Some(left), Some(right)) => self.equivalent(left, right, depth + 1),
                    _ => Ok(false),
                }
            }
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
