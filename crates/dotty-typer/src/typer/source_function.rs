//! Canonical identity lookup for source-level Scala function classes.

use super::*;
use dotty_core::{
    Annotation, ClassInfo, MethodKind, MethodParam, Namespace, Scope, Symbol, SymbolFlags,
    SymbolInfo, SymbolKind, SymbolOrigin, TermName, TypeLambda, TypeName, TypeParam, Variance,
    Visibility,
};

/// The source-level function class family requested by function type syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceFunctionKind {
    /// An ordinary function, represented by `scala.FunctionN`.
    Ordinary,
    /// A contextual function, represented by `scala.ContextFunctionN`.
    Contextual,
}

impl SourceFunctionKind {
    fn class_name(self, arity: usize) -> String {
        let prefix = match self {
            Self::Ordinary => "Function",
            Self::Contextual => "ContextFunction",
        };
        format!("{prefix}{arity}")
    }
}

/// Whether a source function uses the pure/default or capture-checking
/// impure semantic family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceFunctionPurity {
    /// The canonical `FunctionN` or `ContextFunctionN` class family.
    Pure,
    /// The canonical `ImpureFunctionN` or `ImpureContextFunctionN` alias.
    Impure,
}

/// The semantic family selected by source function syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceFunctionFamily {
    pub kind: SourceFunctionKind,
    pub purity: SourceFunctionPurity,
}

impl SourceFunctionFamily {
    pub const fn new(kind: SourceFunctionKind, purity: SourceFunctionPurity) -> Self {
        Self { kind, purity }
    }

    fn alias_name(self, arity: usize) -> String {
        let prefix = match self.kind {
            SourceFunctionKind::Ordinary => "ImpureFunction",
            SourceFunctionKind::Contextual => "ImpureContextFunction",
        };
        format!("{prefix}{arity}")
    }
}

/// Largest source function arity materialized by this typer increment.
///
/// Scala's compiler can synthesize function classes above arity 22 and erases
/// them through `FunctionXXL`; this source typer does not synthesize those
/// classes and therefore reports such requests explicitly.
pub const MAX_SOURCE_FUNCTION_ARITY: usize = 22;

impl SourceTyper<'_> {
    /// Resolves a semantic function family to its canonical type constructor.
    /// Impure families are generic aliases and are kept distinct from the
    /// pure function class symbols returned by [`Self::source_function_class`].
    pub fn source_function_family_type_constructor(
        &mut self,
        family: SourceFunctionFamily,
        arity: usize,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        self.run_atomic(|typer, _| {
            if arity > MAX_SOURCE_FUNCTION_ARITY {
                return Err(TyperError::UnsupportedSourceFunctionArity {
                    kind: family.kind,
                    arity,
                    max_arity: MAX_SOURCE_FUNCTION_ARITY,
                });
            }
            let symbol = match family.purity {
                SourceFunctionPurity::Pure => {
                    typer.source_function_class_inner(family.kind, arity, tree_index)?
                }
                SourceFunctionPurity::Impure => {
                    typer.source_impure_function_alias_inner(family, arity, tree_index)?
                }
            };
            let prefix = typer.type_symbol_prefix(symbol);
            Ok(typer.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(symbol),
            }))
        })
    }

    /// Resolves or materializes the canonical Scala 3.9 impure function alias.
    pub fn source_impure_function_alias(
        &mut self,
        family: SourceFunctionFamily,
        arity: usize,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        self.run_atomic(|typer, _| {
            typer.source_impure_function_alias_inner(family, arity, tree_index)
        })
    }

    fn source_impure_function_alias_inner(
        &mut self,
        family: SourceFunctionFamily,
        arity: usize,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        if family.purity != SourceFunctionPurity::Impure {
            return Err(TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error: ResolutionError::Malformed {
                    reason: "impure function alias lookup requires the impure family".to_owned(),
                },
            });
        }
        if arity > MAX_SOURCE_FUNCTION_ARITY {
            return Err(TyperError::UnsupportedSourceFunctionArity {
                kind: family.kind,
                arity,
                max_arity: MAX_SOURCE_FUNCTION_ARITY,
            });
        }
        let package = match self.packages.symbol(&["scala"]) {
            Some(package) => Some(package),
            None => self.resolve_external_package(&["scala".to_owned()], tree_index)?,
        };
        let Some(package) = package else {
            return Err(TyperError::SourceFunctionClassNotFound {
                kind: family.kind,
                arity,
            });
        };
        let name = Name::new(
            self.store.names.intern(&family.alias_name(arity)),
            Namespace::Type,
        );
        let local_candidates = self.package_type_members(package, name);
        let alias = match local_candidates.as_slice() {
            [alias] => *alias,
            [] => {
                let request = MemberRequest {
                    prefix: self.package_type_prefix(package),
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                match self
                    .resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })? {
                    Some(alias) => alias,
                    None => self.materialize_impure_function_alias(
                        package, family, arity, name, tree_index,
                    )?,
                }
            }
            many => {
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error: ResolutionError::Ambiguous {
                        candidates: many.len(),
                    },
                });
            }
        };
        self.validate_impure_function_alias(alias, package, family, arity, name, tree_index)?;
        Ok(alias)
    }

    fn package_type_members(&self, package: SymbolId, name: Name) -> Vec<SymbolId> {
        self.scopes_of(package)
            .into_iter()
            .flat_map(|scope| {
                self.store
                    .scopes
                    .get(scope)
                    .lookup_all(&name)
                    .iter()
                    .copied()
            })
            .collect()
    }

    fn semantically_equivalent_types(&mut self, left: TypeId, right: TypeId) -> bool {
        matches!(self.is_subtype(left, right), Ok(true))
            && matches!(self.is_subtype(right, left), Ok(true))
    }

    fn is_package_type_prefix(&self, prefix: TypeId, package: SymbolId) -> bool {
        matches!(
            self.store.types.try_get(prefix),
            Some(Type::TypeRef {
                prefix: root,
                target: TypeRefTarget::Symbol(target),
            }) if *root == self.definitions.no_prefix && *target == package
        )
    }

    fn malformed_function_alias(&self, tree_index: u32, name: Name, reason: &str) -> TyperError {
        TyperError::SymbolResolution {
            source: self.source,
            tree_index,
            error: ResolutionError::Malformed {
                reason: format!(
                    "canonical function alias {} is malformed: {reason}",
                    self.store.names.resolve(name.text())
                ),
            },
        }
    }

    fn materialize_impure_function_alias(
        &mut self,
        package: SymbolId,
        family: SourceFunctionFamily,
        arity: usize,
        name: Name,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        let underlying = self.source_function_class_inner(family.kind, arity, tree_index)?;
        let annotation_package = match self.packages.symbol(&["scala", "annotation"]) {
            Some(package) => Some(package),
            None => self.resolve_external_package(
                &["scala".to_owned(), "annotation".to_owned()],
                tree_index,
            )?,
        }
        .ok_or_else(|| {
            self.malformed_function_alias(
                tree_index,
                name,
                "the scala.annotation package containing retainsCap is unavailable",
            )
        })?;
        let annotation_name = Name::new(self.store.names.intern("retainsCap"), Namespace::Type);
        let annotation_request = MemberRequest {
            prefix: self.package_type_prefix(annotation_package),
            name: annotation_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let annotation_candidates = self.package_type_members(annotation_package, annotation_name);
        let annotation_class = match annotation_candidates.as_slice() {
            [annotation_class] => *annotation_class,
            [] => self
                .resolver
                .resolve_member(self.store, &annotation_request)
                .map_err(|error| TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error,
                })?
                .ok_or_else(|| {
                    self.malformed_function_alias(
                        tree_index,
                        name,
                        "the canonical scala.annotation.retainsCap class is unavailable",
                    )
                })?,
            many => {
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error: ResolutionError::Ambiguous {
                        candidates: many.len(),
                    },
                });
            }
        };
        if !self.store.symbols.contains(annotation_class)
            || self.store.symbols.get(annotation_class).name != annotation_name
            || self.store.symbols.get(annotation_class).owner != Some(annotation_package)
            || !matches!(
                self.store.symbols.get(annotation_class).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return Err(self.malformed_function_alias(
                tree_index,
                name,
                "the resolver returned a non-canonical retainsCap class",
            ));
        }

        let bounds = self.store.types.alloc(Type::Bounds {
            low: self.definitions.nothing_type,
            high: self.definitions.any_type,
        });
        let reserved = self.store.types.reserve();
        let mut params = Vec::with_capacity(arity + 1);
        let mut args = Vec::with_capacity(arity + 1);
        for index in 0..=arity {
            let is_result = index == arity;
            let param_name = if is_result {
                "R".to_owned()
            } else {
                format!("T{}", index + 1)
            };
            params.push(TypeParam {
                name: TypeName::new(self.store.names.intern(&param_name)),
                bounds,
                declared_variance: Some(if is_result {
                    Variance::Covariant
                } else {
                    Variance::Contravariant
                }),
            });
            args.push(self.store.types.alloc(Type::ParamRef {
                binder: reserved.id(),
                index: index as u32,
            }));
        }
        let underlying_prefix = self.type_symbol_prefix(underlying);
        let underlying_ref = self.store.types.alloc(Type::TypeRef {
            prefix: underlying_prefix,
            target: TypeRefTarget::Symbol(underlying),
        });
        let applied = self.store.types.alloc(Type::Applied {
            tycon: underlying_ref,
            args,
        });
        let annotation_type = self.type_symbol_prefix(annotation_class);
        let annotation_type = self.store.types.alloc(Type::TypeRef {
            prefix: annotation_type,
            target: TypeRefTarget::Symbol(annotation_class),
        });
        let annotation = self
            .store
            .annotations
            .alloc(Annotation::compact(annotation_type));
        let body = self.store.types.alloc(Type::Annotated {
            underlying: applied,
            annotation,
        });
        let lambda = self.store.types.fill(
            reserved,
            Type::TypeLambda(TypeLambda {
                params,
                result: body,
            }),
        );
        let alias_info = self
            .store
            .types
            .alloc(Type::AliasingBounds { alias: lambda });
        let alias = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(package),
            kind: SymbolKind::TypeAlias,
            flags: SymbolFlags::SYNTHETIC,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(alias_info),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: Default::default(),
        });
        let resolver_entered = self
            .resolver
            .enter_synthetic_package_member(self.store, package, name, alias)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if !resolver_entered {
            let package_scope = self.packages.scope_of(package).ok_or_else(|| {
                self.malformed_function_alias(
                    tree_index,
                    name,
                    "the scala package has no scope for its synthesized alias",
                )
            })?;
            self.store.scopes.get_mut(package_scope).enter(name, alias);
            self.synthetic_package_entries.push((package_scope, alias));
        }
        Ok(alias)
    }

    fn validate_impure_function_alias(
        &mut self,
        alias: SymbolId,
        package: SymbolId,
        family: SourceFunctionFamily,
        arity: usize,
        name: Name,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let malformed = |typer: &SourceTyper<'_>, reason| {
            typer.malformed_function_alias(tree_index, name, reason)
        };
        if !self.store.symbols.contains(alias) {
            return Err(TyperError::UnknownSymbol { symbol: alias });
        }
        let symbol = self.store.symbols.get(alias);
        if symbol.name != name
            || symbol.owner != Some(package)
            || symbol.kind != SymbolKind::TypeAlias
        {
            return Err(malformed(
                self,
                "name, owner, or symbol kind does not match",
            ));
        }
        let SymbolInfo::Complete(info) = symbol.info else {
            return Err(malformed(self, "alias info is missing or incomplete"));
        };
        let Some(Type::AliasingBounds { alias: lambda_id }) = self.store.types.try_get(info) else {
            return Err(malformed(self, "alias info is not AliasingBounds"));
        };
        let lambda_id = *lambda_id;
        let Some(Type::TypeLambda(lambda)) = self.store.types.try_get(lambda_id).cloned() else {
            return Err(malformed(self, "alias body is not a TypeLambda"));
        };
        if lambda.params.len() != arity + 1 {
            return Err(malformed(
                self,
                "generic arity does not match the function arity",
            ));
        }
        for (index, param) in lambda.params.iter().enumerate() {
            let expected_variance = if index == arity {
                Variance::Covariant
            } else {
                Variance::Contravariant
            };
            let bounds_match = match self.store.types.try_get(param.bounds).cloned() {
                Some(Type::Bounds { low, high }) => {
                    self.semantically_equivalent_types(low, self.definitions.nothing_type)
                        && self.semantically_equivalent_types(high, self.definitions.any_type)
                }
                _ => false,
            };
            if param.declared_variance != Some(expected_variance) || !bounds_match {
                return Err(malformed(
                    self,
                    "type parameter bounds or variance are incompatible",
                ));
            }
        }
        let (underlying, annotation) = match self.store.types.try_get(lambda.result) {
            Some(Type::Annotated {
                underlying,
                annotation,
            }) => (*underlying, *annotation),
            _ => {
                return Err(malformed(
                    self,
                    "alias body does not retain the capability annotation",
                ));
            }
        };
        let (tycon, args) = match self.store.types.try_get(underlying) {
            Some(Type::Applied { tycon, args }) => (*tycon, args.clone()),
            _ => {
                return Err(malformed(
                    self,
                    "alias body is not an applied function type",
                ));
            }
        };
        if args.len() != arity + 1 {
            return Err(malformed(
                self,
                "alias body argument count does not match its arity",
            ));
        }
        for (index, argument) in args.iter().enumerate() {
            if !matches!(
                self.store.types.try_get(*argument),
                Some(Type::ParamRef { binder, index: param_index })
                    if *binder == lambda_id && *param_index as usize == index
            ) {
                return Err(malformed(
                    self,
                    "alias body arguments escape or reorder the type lambda",
                ));
            }
        }
        let Type::TypeRef {
            prefix: underlying_prefix,
            target: TypeRefTarget::Symbol(underlying),
        } =
            self.store.types.try_get(tycon).cloned().ok_or_else(|| {
                malformed(self, "alias body has an invalid function type reference")
            })?
        else {
            return Err(malformed(
                self,
                "alias body does not reference a canonical function class",
            ));
        };
        let canonical_underlying =
            self.source_function_class_inner(family.kind, arity, tree_index)?;
        if underlying != canonical_underlying {
            return Err(malformed(
                self,
                "alias body targets the wrong function family",
            ));
        }
        if !self.is_package_type_prefix(underlying_prefix, package) {
            return Err(malformed(
                self,
                "alias body function reference has a non-canonical prefix",
            ));
        }
        let annotation_type = self
            .store
            .annotations
            .try_get(annotation)
            .ok_or_else(|| malformed(self, "alias body has an invalid annotation reference"))?
            .ty;
        let Type::TypeRef {
            prefix: annotation_prefix,
            target: TypeRefTarget::Symbol(annotation_class),
            ..
        } = self
            .store
            .types
            .try_get(annotation_type)
            .cloned()
            .ok_or_else(|| {
                malformed(self, "retainsCap annotation has an invalid type reference")
            })?
        else {
            return Err(malformed(
                self,
                "retainsCap annotation has no canonical class reference",
            ));
        };
        if !self.store.symbols.contains(annotation_class)
            || self.store.symbols.get(annotation_class).name
                != Name::new(self.store.names.intern("retainsCap"), Namespace::Type)
            || !matches!(
                self.store.symbols.get(annotation_class).kind,
                SymbolKind::Class | SymbolKind::Trait
            )
        {
            return Err(malformed(
                self,
                "alias body has a non-canonical retainsCap annotation",
            ));
        }
        let annotation_symbol = self.store.symbols.get(annotation_class);
        let Some(annotation_package) = annotation_symbol.owner else {
            return Err(malformed(
                self,
                "alias body has a retainsCap class without a package owner",
            ));
        };
        if !self.store.symbols.contains(annotation_package)
            || self.package_path(annotation_package) != ["scala", "annotation"]
        {
            return Err(malformed(
                self,
                "alias body has a non-canonical retainsCap annotation",
            ));
        }
        if !self.is_package_type_prefix(annotation_prefix, annotation_package) {
            return Err(malformed(
                self,
                "retainsCap annotation has a non-canonical prefix",
            ));
        }
        let annotation = self.store.annotations.get(annotation);
        if matches!(
            &annotation.arguments,
            dotty_core::AnnotationArguments::Known(arguments) if !arguments.is_empty()
        ) {
            return Err(malformed(
                self,
                "retainsCap annotation must not have term arguments",
            ));
        }
        Ok(())
    }

    /// Resolves the canonical class symbol for a source function kind and arity.
    ///
    /// Resolution reuses the entered `scala` package and the configured
    /// [`SymbolResolver`]. It never manufactures a parallel function symbol.
    /// `tree_index` identifies the source tree that requested the identity for
    /// errors raised by the resolver.
    pub fn source_function_class(
        &mut self,
        kind: SourceFunctionKind,
        arity: usize,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        self.run_atomic(|typer, _| typer.source_function_class_inner(kind, arity, tree_index))
    }

    fn source_function_class_inner(
        &mut self,
        kind: SourceFunctionKind,
        arity: usize,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        if arity > MAX_SOURCE_FUNCTION_ARITY {
            return Err(TyperError::UnsupportedSourceFunctionArity {
                kind,
                arity,
                max_arity: MAX_SOURCE_FUNCTION_ARITY,
            });
        }

        let package = match self.packages.symbol(&["scala"]) {
            Some(package) => Some(package),
            None => self.resolve_external_package(&["scala".to_owned()], tree_index)?,
        };
        let Some(package) = package else {
            return Err(TyperError::SourceFunctionClassNotFound { kind, arity });
        };
        let name = Name::new(
            self.store.names.intern(&kind.class_name(arity)),
            dotty_core::Namespace::Type,
        );

        let local_candidates: Vec<_> = self
            .scopes_of(package)
            .into_iter()
            .flat_map(|scope| {
                self.store
                    .scopes
                    .get(scope)
                    .lookup_all(&name)
                    .iter()
                    .copied()
            })
            .filter(|symbol| {
                self.store.symbols.get(*symbol).owner == Some(package)
                    && matches!(
                        self.store.symbols.get(*symbol).kind,
                        SymbolKind::Class | SymbolKind::Trait
                    )
            })
            .collect();
        let class = match local_candidates.as_slice() {
            [class] => *class,
            [] => {
                let request = MemberRequest {
                    prefix: self.package_type_prefix(package),
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                };
                match self
                    .resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })? {
                    Some(class) => class,
                    None if kind == SourceFunctionKind::Contextual => {
                        self.materialize_context_function(package, name, arity, tree_index)?
                    }
                    None => {
                        return Err(TyperError::SourceFunctionClassNotFound { kind, arity });
                    }
                }
            }
            many => {
                return Err(TyperError::SymbolResolution {
                    source: self.source,
                    tree_index,
                    error: ResolutionError::Ambiguous {
                        candidates: many.len(),
                    },
                });
            }
        };

        if !self.store.symbols.contains(class) {
            return Err(TyperError::UnknownSymbol { symbol: class });
        }
        let symbol = self.store.symbols.get(class);
        if symbol.name != name
            || symbol.owner != Some(package)
            || !matches!(symbol.kind, SymbolKind::Class | SymbolKind::Trait)
        {
            return Err(TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error: ResolutionError::Malformed {
                    reason: format!(
                        "source function lookup for {} returned a non-canonical class symbol",
                        kind.class_name(arity)
                    ),
                },
            });
        }
        Ok(class)
    }

    /// Dotty 3.9 installs a synthesizer on `scala` for `ContextFunctionN`:
    /// these traits have no classpath files. Keep the synthesized declaration
    /// in the shared semantic store and package scope so every later lookup
    /// observes the same canonical symbol.
    fn materialize_context_function(
        &mut self,
        package: SymbolId,
        class_name: Name,
        arity: usize,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        let class = self.store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::SYNTHETIC | SymbolFlags::ABSTRACT,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: Default::default(),
        });
        let declarations = self.store.scopes.alloc(Scope::new(Some(class)));
        let class_prefix = self.store.types.alloc(Type::ThisType { class });
        let mut param_refs = Vec::with_capacity(arity + 1);

        for index in 0..=arity {
            let param_text = if index == arity {
                "R".to_owned()
            } else {
                format!("T{}", index + 1)
            };
            let param_name = Name::new(self.store.names.intern(&param_text), Namespace::Type);
            let param = self.store.symbols.alloc(Symbol {
                name: param_name,
                owner: Some(class),
                kind: SymbolKind::TypeParameter,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Complete(self.store.types.alloc(Type::Bounds {
                    low: self.definitions.nothing_type,
                    high: self.definitions.any_type,
                })),
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: Default::default(),
            });
            self.store
                .scopes
                .get_mut(declarations)
                .enter(param_name, param);
            param_refs.push(self.store.types.alloc(Type::TypeRef {
                prefix: class_prefix,
                target: TypeRefTarget::Symbol(param),
            }));
        }

        let result = *param_refs
            .last()
            .expect("function type always has a result parameter");
        let method_name = Name::new(self.store.names.intern("apply"), Namespace::Term);
        let method_type = self.store.types.alloc(Type::Method(MethodType {
            params: param_refs[..arity]
                .iter()
                .enumerate()
                .map(|(index, ty)| MethodParam {
                    name: TermName::new(self.store.names.intern(&format!("x{index}"))),
                    ty: *ty,
                    erased: false,
                    varargs: false,
                })
                .collect(),
            result,
            kind: MethodKind::Contextual,
        }));
        let apply = self.store.symbols.alloc(Symbol {
            name: method_name,
            owner: Some(class),
            kind: SymbolKind::Method,
            flags: SymbolFlags::ABSTRACT,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(method_type),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: Default::default(),
        });
        self.store
            .scopes
            .get_mut(declarations)
            .enter(method_name, apply);
        let package_prefix = self.package_type_prefix(package);
        let class_info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: package_prefix,
            class,
            parents: vec![self.definitions.object_type],
            declarations,
            self_type: None,
        }));
        self.store
            .symbols
            .set_info(class, SymbolInfo::Complete(class_info));
        let resolver_entered = self
            .resolver
            .enter_synthetic_package_member(self.store, package, class_name, class)
            .map_err(|error| TyperError::SymbolResolution {
                source: self.source,
                tree_index,
                error,
            })?;
        if !resolver_entered {
            let package_scope =
                self.packages
                    .scope_of(package)
                    .ok_or(TyperError::SourceFunctionClassNotFound {
                        kind: SourceFunctionKind::Contextual,
                        arity,
                    })?;
            self.store
                .scopes
                .get_mut(package_scope)
                .enter(class_name, class);
            self.synthetic_package_entries.push((package_scope, class));
        }
        Ok(class)
    }

    /// Resolves a source function class and returns its semantic type reference.
    pub fn source_function_type_constructor(
        &mut self,
        kind: SourceFunctionKind,
        arity: usize,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        self.run_atomic(|typer, _| {
            let class = typer.source_function_class_inner(kind, arity, tree_index)?;
            let prefix = typer.type_symbol_prefix(class);
            Ok(typer.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(class),
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{
        Name, Namespace, SourceSemanticIndex, Symbol, SymbolFlags, SymbolInfo, SymbolLinks,
        SymbolOrigin, Visibility,
    };
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    #[derive(Default)]
    struct FunctionResolver {
        package: Option<SymbolId>,
        annotation_package: Option<SymbolId>,
        classes: HashMap<Name, SymbolId>,
        requests: Rc<RefCell<Vec<MemberRequest>>>,
    }

    impl SymbolResolver for FunctionResolver {
        fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
            dotty_core::ResolverCheckpoint::new(self.requests.borrow().len() as u64)
        }

        fn rollback_to(
            &mut self,
            _store: &mut SemanticStore,
            checkpoint: dotty_core::ResolverCheckpoint,
        ) {
            self.requests
                .borrow_mut()
                .truncate(checkpoint.token() as usize);
        }

        fn resolve_member(
            &mut self,
            _store: &mut SemanticStore,
            request: &MemberRequest,
        ) -> Result<Option<SymbolId>, ResolutionError> {
            self.requests.borrow_mut().push(request.clone());
            Ok(self.classes.get(&request.name).copied())
        }

        fn resolve_package(
            &mut self,
            _store: &mut SemanticStore,
            path: &[&str],
        ) -> Result<Option<SymbolId>, ResolutionError> {
            Ok(match path {
                ["scala"] => self.package,
                ["scala", "annotation"] => self.annotation_package,
                _ => None,
            })
        }
    }

    fn function_class(
        store: &mut SemanticStore,
        package: SymbolId,
        name: &str,
    ) -> (Name, SymbolId) {
        let name = Name::new(store.names.intern(name), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(package),
            kind: SymbolKind::Trait,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Tasty(store.origins.register_tasty()),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        (name, symbol)
    }

    fn add_retains_cap(
        store: &mut SemanticStore,
        packages: &mut Packages,
        scala: SymbolId,
    ) -> (SymbolId, Name, SymbolId) {
        let annotation_package = packages
            .enter(store, SymbolOrigin::Synthetic, &["scala", "annotation"])
            .last()
            .expect("scala.annotation package")
            .symbol;
        let (name, symbol) = function_class(store, annotation_package, "retainsCap");
        let scope = packages
            .scope_of(annotation_package)
            .expect("package scope");
        store.scopes.get_mut(scope).enter(name, symbol);
        assert_eq!(store.symbols.get(scala).kind, SymbolKind::Package);
        (annotation_package, name, symbol)
    }

    fn setup() -> (
        AstArena<Untyped>,
        SourceSemanticIndex,
        SemanticStore,
        Packages,
        Definitions,
        SymbolId,
    ) {
        let arena = AstArena::new();
        let index = SourceSemanticIndex::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let scala = packages
            .enter(&mut store, SymbolOrigin::Synthetic, &["scala"])
            .last()
            .expect("scala package")
            .symbol;
        (arena, index, store, packages, definitions, scala)
    }

    fn typer<'a>(
        arena: &'a AstArena<Untyped>,
        index: &'a SourceSemanticIndex,
        store: &'a mut SemanticStore,
        packages: &'a Packages,
        definitions: Definitions,
        resolver: FunctionResolver,
    ) -> SourceTyper<'a> {
        SourceTyper::new(
            arena,
            dotty_core::SourceId::from_index(1),
            index,
            store,
            definitions,
            packages,
        )
        .with_resolver(Box::new(resolver))
    }

    fn impure_alias_symbol(
        store: &mut SemanticStore,
        package: SymbolId,
        annotation_class: SymbolId,
        underlying: SymbolId,
        name: Name,
        arity: usize,
        definitions: Definitions,
    ) -> SymbolId {
        let low = store
            .types
            .alloc(store.types.get(definitions.nothing_type).clone());
        let high = store
            .types
            .alloc(store.types.get(definitions.any_type).clone());
        let bounds = store.types.alloc(Type::Bounds { low, high });
        let reserved = store.types.reserve();
        let mut params = Vec::new();
        let mut args = Vec::new();
        for index in 0..=arity {
            let is_result = index == arity;
            let param_name = if is_result {
                "R".to_owned()
            } else {
                format!("T{}", index + 1)
            };
            params.push(TypeParam {
                name: TypeName::new(store.names.intern(&param_name)),
                bounds,
                declared_variance: Some(if is_result {
                    Variance::Covariant
                } else {
                    Variance::Contravariant
                }),
            });
            args.push(store.types.alloc(Type::ParamRef {
                binder: reserved.id(),
                index: index as u32,
            }));
        }
        let package_prefix = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(package),
        });
        let prefix = store.types.alloc(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(underlying),
        });
        let applied = store.types.alloc(Type::Applied {
            tycon: prefix,
            args,
        });
        let annotation_prefix = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(
                store
                    .symbols
                    .get(annotation_class)
                    .owner
                    .expect("annotation owner"),
            ),
        });
        let annotation_type = store.types.alloc(Type::TypeRef {
            prefix: annotation_prefix,
            target: TypeRefTarget::Symbol(annotation_class),
        });
        let annotation = store
            .annotations
            .alloc(Annotation::compact(annotation_type));
        let result = store.types.alloc(Type::Annotated {
            underlying: applied,
            annotation,
        });
        let lambda = store
            .types
            .fill(reserved, Type::TypeLambda(TypeLambda { params, result }));
        let info = store.types.alloc(Type::AliasingBounds { alias: lambda });
        store.symbols.alloc(Symbol {
            name,
            owner: Some(package),
            kind: SymbolKind::TypeAlias,
            flags: SymbolFlags::SYNTHETIC,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(info),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn resolver_with_function_classes(
        store: &mut SemanticStore,
        scala: SymbolId,
        annotation_package: SymbolId,
        annotation_name: Name,
        annotation_class: SymbolId,
        arities: &[usize],
    ) -> FunctionResolver {
        let mut classes = HashMap::from([(annotation_name, annotation_class)]);
        for arity in arities {
            for kind in [SourceFunctionKind::Ordinary, SourceFunctionKind::Contextual] {
                let (name, symbol) = function_class(store, scala, &kind.class_name(*arity));
                classes.insert(name, symbol);
            }
        }
        FunctionResolver {
            package: Some(scala),
            annotation_package: Some(annotation_package),
            classes,
            ..FunctionResolver::default()
        }
    }

    fn resolver_alias_fixture() -> (
        AstArena<Untyped>,
        SourceSemanticIndex,
        SemanticStore,
        Packages,
        Definitions,
        SymbolId,
        FunctionResolver,
        SymbolId,
        TypeId,
        dotty_core::AnnotationId,
    ) {
        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let mut resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let (underlying_name, underlying) = function_class(&mut store, scala, "Function1");
        resolver.classes.insert(underlying_name, underlying);
        let alias_name = Name::new(store.names.intern("ImpureFunction1"), Namespace::Type);
        let alias = impure_alias_symbol(
            &mut store,
            scala,
            annotation_class,
            underlying,
            alias_name,
            1,
            definitions,
        );
        resolver.classes.insert(alias_name, alias);
        let info = match store.symbols.get(alias).info {
            SymbolInfo::Complete(info) => info,
            _ => unreachable!(),
        };
        let lambda = match store.types.get(info) {
            Type::AliasingBounds { alias } => *alias,
            _ => unreachable!(),
        };
        let result = match store.types.get(lambda) {
            Type::TypeLambda(lambda) => lambda.result,
            _ => unreachable!(),
        };
        let (underlying, annotation) = match store.types.get(result) {
            Type::Annotated {
                underlying,
                annotation,
            } => (*underlying, *annotation),
            _ => unreachable!(),
        };
        let tycon = match store.types.get(underlying) {
            Type::Applied { tycon, .. } => *tycon,
            _ => unreachable!(),
        };
        (
            arena,
            index,
            store,
            packages,
            definitions,
            scala,
            resolver,
            alias,
            tycon,
            annotation,
        )
    }

    #[test]
    fn resolves_ordinary_function_arities_and_reuses_external_symbol_identity() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let mut classes = HashMap::new();
        let mut expected = Vec::new();
        for arity in [0, 1, 2, MAX_SOURCE_FUNCTION_ARITY] {
            let (name, symbol) = function_class(&mut store, scala, &format!("Function{arity}"));
            classes.insert(name, symbol);
            expected.push(symbol);
        }
        let requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = FunctionResolver {
            package: Some(scala),
            classes,
            requests: Rc::clone(&requests),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        for (arity, expected) in [0, 1, 2, MAX_SOURCE_FUNCTION_ARITY]
            .into_iter()
            .zip(expected)
        {
            let first = typer
                .source_function_class(SourceFunctionKind::Ordinary, arity, 7)
                .unwrap();
            let second = typer
                .source_function_class(SourceFunctionKind::Ordinary, arity, 8)
                .unwrap();
            assert_eq!(first, expected);
            assert_eq!(second, first);
            assert!(matches!(
                typer.store.symbols.get(first).origin,
                SymbolOrigin::Tasty(_)
            ));
        }

        let requests = requests.borrow();
        assert_eq!(requests.len(), 8);
        assert!(requests.iter().all(|request| {
            request.space == MemberSpace::Prefix
                && matches!(
                    typer.store.types.get(request.prefix),
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(package),
                        ..
                    } if *package == scala
                )
        }));
    }

    #[test]
    fn already_entered_function_class_is_reused_without_external_resolution() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let (name, function) = function_class(&mut store, scala, "Function1");
        let scope = packages.scope_of(scala).expect("scala package scope");
        store.scopes.get_mut(scope).enter(name, function);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = FunctionResolver {
            package: Some(scala),
            requests: Rc::clone(&requests),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert_eq!(
            typer
                .source_function_class(SourceFunctionKind::Ordinary, 1, 5)
                .unwrap(),
            function
        );
        assert_eq!(
            typer
                .source_function_class(SourceFunctionKind::Ordinary, 1, 6)
                .unwrap(),
            function
        );
        assert!(requests.borrow().is_empty());
    }

    #[test]
    fn contextual_and_ordinary_functions_keep_distinct_class_identities() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let (ordinary_name, ordinary) = function_class(&mut store, scala, "Function1");
        let (contextual_name, contextual) = function_class(&mut store, scala, "ContextFunction1");
        let resolver = FunctionResolver {
            package: Some(scala),
            classes: HashMap::from([(ordinary_name, ordinary), (contextual_name, contextual)]),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert_eq!(
            typer
                .source_function_class(SourceFunctionKind::Ordinary, 1, 3)
                .unwrap(),
            ordinary
        );
        assert_eq!(
            typer
                .source_function_class(SourceFunctionKind::Contextual, 1, 4)
                .unwrap(),
            contextual
        );
        assert_ne!(ordinary, contextual);
    }

    #[test]
    fn type_constructor_is_a_type_ref_to_the_resolved_package_class() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let (name, function) = function_class(&mut store, scala, "Function2");
        let resolver = FunctionResolver {
            package: Some(scala),
            classes: HashMap::from([(name, function)]),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        let constructor = typer
            .source_function_type_constructor(SourceFunctionKind::Ordinary, 2, 9)
            .unwrap();
        let Type::TypeRef { prefix, target } = typer.store.types.get(constructor) else {
            panic!("function constructor should be a type reference")
        };
        assert_eq!(*target, TypeRefTarget::Symbol(function));
        assert!(matches!(
            typer.store.types.get(*prefix),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(package),
                ..
            } if *package == scala
        ));
    }

    #[test]
    fn all_function_families_are_distinct_and_impure_arities_reuse_aliases() {
        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[0, 1, 2, MAX_SOURCE_FUNCTION_ARITY],
        );
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        for kind in [SourceFunctionKind::Ordinary, SourceFunctionKind::Contextual] {
            for arity in [0, 1, 2, MAX_SOURCE_FUNCTION_ARITY] {
                let family = SourceFunctionFamily::new(kind, SourceFunctionPurity::Impure);
                let alias = typer
                    .source_impure_function_alias(family, arity, 31)
                    .unwrap();
                assert_eq!(typer.store.symbols.get(alias).owner, Some(scala));
                assert_eq!(typer.store.symbols.get(alias).kind, SymbolKind::TypeAlias);
                let SymbolInfo::Complete(info) = typer.store.symbols.get(alias).info else {
                    panic!("canonical impure function alias has complete info");
                };
                let Type::AliasingBounds { alias: lambda } = typer.store.types.get(info) else {
                    panic!("canonical impure function alias uses AliasingBounds");
                };
                let Type::TypeLambda(lambda_info) = typer.store.types.get(*lambda) else {
                    panic!("canonical impure function alias binds its parameters");
                };
                assert_eq!(lambda_info.params.len(), arity + 1);
                assert!(
                    lambda_info.params[..arity]
                        .iter()
                        .all(|param| param.declared_variance == Some(Variance::Contravariant))
                );
                assert_eq!(
                    lambda_info.params[arity].declared_variance,
                    Some(Variance::Covariant)
                );
                let Type::Annotated { underlying, .. } = typer.store.types.get(lambda_info.result)
                else {
                    panic!("canonical impure alias retains RetainsCap");
                };
                let Type::Applied { args, .. } = typer.store.types.get(*underlying) else {
                    panic!("canonical impure alias applies its function class");
                };
                assert_eq!(args.len(), arity + 1);
                for (index, argument) in args.iter().enumerate() {
                    assert!(matches!(
                        typer.store.types.get(*argument),
                        Type::ParamRef { binder, index: parameter }
                            if *binder == *lambda && *parameter as usize == index
                    ));
                }
                assert_eq!(
                    typer
                        .source_impure_function_alias(family, arity, 32)
                        .unwrap(),
                    alias
                );
                let constructor = typer
                    .source_function_family_type_constructor(family, arity, 33)
                    .unwrap();
                assert!(matches!(
                    typer.store.types.get(constructor),
                    Type::TypeRef {
                        target: TypeRefTarget::Symbol(target),
                        ..
                    } if *target == alias
                ));
            }
        }

        let ordinary = typer
            .source_function_family_type_constructor(
                SourceFunctionFamily::new(SourceFunctionKind::Ordinary, SourceFunctionPurity::Pure),
                1,
                34,
            )
            .unwrap();
        let contextual = typer
            .source_function_family_type_constructor(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Contextual,
                    SourceFunctionPurity::Pure,
                ),
                1,
                35,
            )
            .unwrap();
        let ordinary_impure = typer
            .source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                36,
            )
            .unwrap();
        let contextual_impure = typer
            .source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Contextual,
                    SourceFunctionPurity::Impure,
                ),
                1,
                37,
            )
            .unwrap();
        let identity = |typer: &SourceTyper<'_>, ty: TypeId| match typer.store.types.get(ty) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            _ => panic!("function family constructor must be a symbol reference"),
        };
        let identities = [
            identity(&typer, ordinary),
            identity(&typer, contextual),
            ordinary_impure,
            contextual_impure,
        ];
        assert_eq!(
            identities
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );
    }

    #[test]
    fn resolver_provided_impure_alias_is_reused_and_malformed_alias_is_rejected() {
        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let mut resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let (underlying_name, underlying) = function_class(&mut store, scala, "Function1");
        resolver.classes.insert(underlying_name, underlying);
        let alias_name = Name::new(store.names.intern("ImpureFunction1"), Namespace::Type);
        let alias = impure_alias_symbol(
            &mut store,
            scala,
            annotation_class,
            underlying,
            alias_name,
            1,
            definitions,
        );
        resolver.classes.insert(alias_name, alias);
        let mut ty = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert_eq!(
            ty.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                41,
            )
            .unwrap(),
            alias
        );

        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let mut resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let alias_name = Name::new(store.names.intern("ImpureFunction1"), Namespace::Type);
        let (_, malformed) = function_class(&mut store, scala, "ImpureFunction1");
        resolver.classes.insert(alias_name, malformed);
        let mut wrong_kind_typer =
            typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert!(matches!(
            wrong_kind_typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                42,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));

        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let mut resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let (underlying_name, underlying) = function_class(&mut store, scala, "Function1");
        resolver.classes.insert(underlying_name, underlying);
        let alias_name = Name::new(store.names.intern("ImpureFunction1"), Namespace::Type);
        let malformed_info = store.symbols.alloc(Symbol {
            name: alias_name,
            owner: Some(scala),
            kind: SymbolKind::TypeAlias,
            flags: SymbolFlags::SYNTHETIC,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        resolver.classes.insert(alias_name, malformed_info);
        let mut malformed_typer =
            typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert!(matches!(
            malformed_typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                45,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));

        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let mut resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let (underlying_name, underlying) = function_class(&mut store, scala, "Function1");
        resolver.classes.insert(underlying_name, underlying);
        let alias_name = Name::new(store.names.intern("ImpureFunction1"), Namespace::Type);
        let wrong_arity = impure_alias_symbol(
            &mut store,
            scala,
            annotation_class,
            underlying,
            alias_name,
            0,
            definitions,
        );
        resolver.classes.insert(alias_name, wrong_arity);
        let mut wrong_arity_typer =
            typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert!(matches!(
            wrong_arity_typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                46,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));
    }

    #[test]
    fn wrong_owner_scope_entry_is_rejected_without_synthesizing_a_duplicate() {
        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let mut resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let (underlying_name, underlying) = function_class(&mut store, scala, "Function1");
        resolver.classes.insert(underlying_name, underlying);
        let alias_name = Name::new(store.names.intern("ImpureFunction1"), Namespace::Type);
        let foreign_owner = packages
            .enter(&mut store, SymbolOrigin::Synthetic, &["other"])
            .last()
            .unwrap()
            .symbol;
        let wrong_owner = store.symbols.alloc(Symbol {
            name: alias_name,
            owner: Some(foreign_owner),
            kind: SymbolKind::TypeAlias,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let scope = packages.scope_of(scala).unwrap();
        store.scopes.get_mut(scope).enter(alias_name, wrong_owner);
        let mut checked_typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert!(matches!(
            checked_typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                47,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));
        assert_eq!(
            checked_typer
                .store
                .scopes
                .get(scope)
                .lookup_all(&alias_name),
            &[wrong_owner]
        );
    }

    #[test]
    fn alias_validation_rejects_noncanonical_annotation_shape() {
        let (arena, index, mut store, packages, definitions, _, resolver, _alias, tycon, _) =
            resolver_alias_fixture();
        let wrong_prefix = store.types.alloc(Type::NoPrefix);
        if let Type::TypeRef { prefix, .. } = store.types.get_mut(tycon) {
            *prefix = wrong_prefix;
        }
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert!(matches!(
            typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                48,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));
    }

    #[test]
    fn alias_validation_rejects_wrong_annotation_namespace_and_arguments() {
        let (arena, index, mut store, packages, definitions, _, resolver, _, _, annotation) =
            resolver_alias_fixture();
        let annotation_ty = store.annotations.get(annotation).ty;
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(annotation_class),
            ..
        } = *store.types.get(annotation_ty)
        else {
            unreachable!()
        };
        store.symbols.get_mut(annotation_class).name =
            Name::new(store.names.intern("retainsCap"), Namespace::Term);
        let mut namespace_typer =
            typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert!(matches!(
            namespace_typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                50,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));

        let (arena, index, mut store, packages, definitions, _, resolver, _, _, annotation) =
            resolver_alias_fixture();
        let annotation_ty = store.annotations.get(annotation).ty;
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(annotation_class),
            ..
        } = *store.types.get(annotation_ty)
        else {
            unreachable!()
        };
        store.symbols.get_mut(annotation_class).kind = SymbolKind::Object;
        let mut kind_typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert!(matches!(
            kind_typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                52,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));

        let (arena, index, mut store, packages, definitions, _, resolver, alias, _, annotation) =
            resolver_alias_fixture();
        let annotation_ty = store.annotations.get(annotation).ty;
        let replacement = store.annotations.alloc(Annotation::with_arguments(
            annotation_ty,
            vec![dotty_core::AnnotationArgument {
                name: None,
                value: dotty_core::AnnotationValue::Constant(dotty_core::Constant::Int(1)),
            }],
        ));
        let info = match store.symbols.get(alias).info {
            SymbolInfo::Complete(info) => info,
            _ => unreachable!(),
        };
        let lambda = match store.types.get(info) {
            Type::AliasingBounds { alias } => *alias,
            _ => unreachable!(),
        };
        let result = match store.types.get(lambda) {
            Type::TypeLambda(lambda) => lambda.result,
            _ => unreachable!(),
        };
        if let Type::Annotated { annotation, .. } = store.types.get_mut(result) {
            *annotation = replacement;
        }
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert!(matches!(
            typer.source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                51,
            ),
            Err(TyperError::SymbolResolution { .. })
        ));
    }

    #[test]
    fn alias_validation_accepts_fresh_equivalent_bounds() {
        let (arena, index, mut store, packages, definitions, _, resolver, alias, _, _) =
            resolver_alias_fixture();
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        assert_eq!(
            typer
                .source_impure_function_alias(
                    SourceFunctionFamily::new(
                        SourceFunctionKind::Ordinary,
                        SourceFunctionPurity::Impure,
                    ),
                    1,
                    49,
                )
                .unwrap(),
            alias
        );
    }

    #[test]
    fn applied_impure_function_aliases_normalize_to_their_function_families() {
        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let ordinary_name = Name::new(store.names.intern("Function1"), Namespace::Type);
        let contextual_name = Name::new(store.names.intern("ContextFunction1"), Namespace::Type);
        let ordinary = resolver.classes[&ordinary_name];
        let contextual = resolver.classes[&contextual_name];
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        let arguments = [definitions.int, definitions.boolean];

        for (kind, expected_class) in [
            (SourceFunctionKind::Ordinary, ordinary),
            (SourceFunctionKind::Contextual, contextual),
        ] {
            let alias = typer
                .source_impure_function_alias(
                    SourceFunctionFamily::new(kind, SourceFunctionPurity::Impure),
                    1,
                    53,
                )
                .unwrap();
            let alias_prefix = typer.type_symbol_prefix(alias);
            let alias_ref = typer.store.types.alloc(Type::TypeRef {
                prefix: alias_prefix,
                target: TypeRefTarget::Symbol(alias),
            });
            let applied = typer.store.types.alloc(Type::Applied {
                tycon: alias_ref,
                args: arguments.to_vec(),
            });

            let normalized = crate::types::TypeNormalizer::new(typer.store)
                .dealias_top(applied)
                .unwrap();

            let Type::Annotated { underlying, .. } = typer.store.types.get(normalized) else {
                panic!("impure function alias normalization keeps RetainsCap metadata")
            };
            let Type::Applied { tycon, args } = typer.store.types.get(*underlying) else {
                panic!("impure alias body should be an applied function type")
            };
            assert!(matches!(
                typer.store.types.get(*tycon),
                Type::TypeRef { target: TypeRefTarget::Symbol(found), .. }
                    if *found == expected_class
            ));
            assert_eq!(args, &arguments);
        }
    }

    #[test]
    fn failed_outer_transaction_rolls_back_impure_alias_and_retry_is_canonical() {
        let (arena, index, mut store, mut packages, definitions, scala) = setup();
        let (annotation_package, annotation_name, annotation_class) =
            add_retains_cap(&mut store, &mut packages, scala);
        let resolver = resolver_with_function_classes(
            &mut store,
            scala,
            annotation_package,
            annotation_name,
            annotation_class,
            &[1],
        );
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        let name = Name::new(typer.store.names.intern("ImpureFunction1"), Namespace::Type);
        let package_scope = packages.scope_of(scala).unwrap();
        let mut allocated_alias = None;
        let mut allocated_info = None;
        let failed: Result<(), TyperError> = typer.run_atomic(|typer, _| {
            let alias = typer.source_impure_function_alias_inner(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                43,
            )?;
            allocated_alias = Some(alias);
            allocated_info = match typer.store.symbols.get(alias).info {
                SymbolInfo::Complete(info) => Some(info),
                _ => None,
            };
            Err(TyperError::UnknownSymbol { symbol: alias })
        });
        assert!(matches!(failed, Err(TyperError::UnknownSymbol { .. })));
        assert!(!typer.store.symbols.contains(allocated_alias.unwrap()));
        assert!(typer.store.types.try_get(allocated_info.unwrap()).is_none());
        assert!(
            typer
                .store
                .scopes
                .get(package_scope)
                .lookup_all(&name)
                .is_empty()
        );
        let alias = typer
            .source_impure_function_alias(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                1,
                44,
            )
            .unwrap();
        assert_eq!(
            typer.store.scopes.get(package_scope).lookup_all(&name),
            &[alias]
        );
    }

    #[test]
    fn missing_ordinary_classpath_function_is_reported_and_failed_resolution_rolls_back() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = FunctionResolver {
            package: Some(scala),
            requests: Rc::clone(&requests),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert!(matches!(
            typer.source_function_class(SourceFunctionKind::Ordinary, 3, 12),
            Err(TyperError::SourceFunctionClassNotFound {
                kind: SourceFunctionKind::Ordinary,
                arity: 3
            })
        ));
        assert!(requests.borrow().is_empty());
    }

    #[test]
    fn missing_contextual_class_is_materialized_once_with_apply_signature() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = FunctionResolver {
            package: Some(scala),
            requests: Rc::clone(&requests),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        let class = typer
            .source_function_class(SourceFunctionKind::Contextual, 2, 15)
            .unwrap();
        let repeated = typer
            .source_function_class(SourceFunctionKind::Contextual, 2, 16)
            .unwrap();
        assert_eq!(class, repeated);
        let symbol = typer.store.symbols.get(class);
        assert_eq!(symbol.origin, SymbolOrigin::Synthetic);
        assert_eq!(symbol.owner, Some(scala));
        assert_eq!(symbol.kind, SymbolKind::Trait);
        let SymbolInfo::Complete(class_info) = symbol.info else {
            panic!("synthesized contextual function has complete class info");
        };
        let Type::ClassInfo(class_info) = typer.store.types.get(class_info) else {
            panic!("synthesized contextual function has ClassInfo");
        };
        let apply_name = Name::new(typer.store.names.intern("apply"), Namespace::Term);
        let [apply] = typer
            .store
            .scopes
            .get(class_info.declarations)
            .lookup_all(&apply_name)
        else {
            panic!("contextual function exposes one apply method");
        };
        let SymbolInfo::Complete(apply_info) = typer.store.symbols.get(*apply).info else {
            panic!("apply method has a complete signature");
        };
        let Type::Method(method) = typer.store.types.get(apply_info) else {
            panic!("apply signature is a method type");
        };
        assert_eq!(method.kind, MethodKind::Contextual);
        assert_eq!(method.params.len(), 2);
        assert!(matches!(
            typer.store.types.get(method.result),
            Type::TypeRef { .. }
        ));
        assert_eq!(requests.borrow().len(), 1);
    }

    #[test]
    fn failed_outer_transaction_removes_synthetic_package_entry_before_retry() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let resolver = FunctionResolver {
            package: Some(scala),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);
        let class_name = Name::new(
            typer.store.names.intern("ContextFunction1"),
            Namespace::Type,
        );
        let package_scope = packages.scope_of(scala).unwrap();

        let failed: Result<(), TyperError> = typer.run_atomic(|typer, _| {
            let class = typer.source_function_class_inner(SourceFunctionKind::Contextual, 1, 18)?;
            Err(TyperError::UnknownSymbol { symbol: class })
        });
        assert!(matches!(failed, Err(TyperError::UnknownSymbol { .. })));
        assert!(
            typer
                .store
                .scopes
                .get(package_scope)
                .lookup_all(&class_name)
                .is_empty()
        );

        let retry = typer
            .source_function_class(SourceFunctionKind::Contextual, 1, 19)
            .unwrap();
        assert!(typer.store.symbols.contains(retry));
        assert_eq!(
            typer
                .store
                .scopes
                .get(package_scope)
                .lookup_all(&class_name),
            &[retry]
        );
    }

    #[test]
    #[ignore = "requires pinned Scala 3.9 jars and JAVA_HOME"]
    fn scala_39_classpath_uses_synthesis_for_context_function_classes() {
        let classpath = std::env::var("SCALA39_CLASSPATH")
            .expect("SCALA39_CLASSPATH must list the pinned Scala 3.9 jars");
        let java_home = std::env::var("JAVA_HOME").expect("JAVA_HOME must identify a JDK");
        let release = std::env::var("SCALA39_JDK_RELEASE")
            .expect("SCALA39_JDK_RELEASE must select the JDK classfile release")
            .parse::<u16>()
            .expect("SCALA39_JDK_RELEASE must be numeric");
        use dotty_classloader::classloader::{
            ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, JarClassPath,
            JdkClassPath, LoadingSession,
        };
        use std::path::PathBuf;

        let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
            JdkClassPath::new(PathBuf::from(java_home).join("jmods")).unwrap(),
        )];
        for jar in std::env::split_paths(&classpath) {
            entries.push(Box::new(JarClassPath::new(jar, release).unwrap()));
        }
        let classpath = CompositeClassPath::new(entries);
        let arena = AstArena::new();
        let index = SourceSemanticIndex::new();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let resolver_packages = Packages::new();
        let typer_packages = Packages::new();
        let resolver = ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::with_packages(resolver_packages),
        );
        let mut typer = SourceTyper::new(
            &arena,
            dotty_core::SourceId::from_index(1),
            &index,
            &mut store,
            definitions,
            &typer_packages,
        )
        .with_resolver(Box::new(resolver));

        let failed: Result<(), TyperError> = typer.run_atomic(|typer, _| {
            let class = typer.source_function_class_inner(SourceFunctionKind::Contextual, 1, 17)?;
            Err(TyperError::UnknownSymbol { symbol: class })
        });
        assert!(matches!(failed, Err(TyperError::UnknownSymbol { .. })));
        let contextual = typer
            .source_function_class(SourceFunctionKind::Contextual, 1, 21)
            .unwrap();
        let repeated = typer
            .source_function_class(SourceFunctionKind::Contextual, 1, 20)
            .unwrap();
        assert_eq!(contextual, repeated);
        let scala = typer.store.symbols.get(contextual).owner.unwrap();
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(scala).name.text()),
            "scala"
        );
        assert_eq!(
            typer.store.symbols.get(contextual).origin,
            SymbolOrigin::Synthetic
        );
        assert!(typer.store.symbols.contains(contextual));
    }

    #[test]
    fn unsupported_excessive_arity_is_focused_and_does_not_resolve() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = FunctionResolver {
            package: Some(scala),
            requests: Rc::clone(&requests),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert!(matches!(
            typer.source_function_class(
                SourceFunctionKind::Ordinary,
                MAX_SOURCE_FUNCTION_ARITY + 1,
                13
            ),
            Err(TyperError::UnsupportedSourceFunctionArity {
                kind: SourceFunctionKind::Ordinary,
                arity: 23,
                max_arity: 22
            })
        ));
        assert!(requests.borrow().is_empty());

        assert!(matches!(
            typer.source_function_family_type_constructor(
                SourceFunctionFamily::new(
                    SourceFunctionKind::Ordinary,
                    SourceFunctionPurity::Impure,
                ),
                MAX_SOURCE_FUNCTION_ARITY + 1,
                24,
            ),
            Err(TyperError::UnsupportedSourceFunctionArity {
                kind: SourceFunctionKind::Ordinary,
                arity: 23,
                max_arity: 22
            })
        ));
        assert!(requests.borrow().is_empty());
    }

    #[test]
    fn absent_scala_package_has_the_same_focused_missing_identity_error() {
        let (arena, index, mut store, packages, definitions, _) = setup();
        let resolver = FunctionResolver::default();
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert!(matches!(
            typer.source_function_class(SourceFunctionKind::Ordinary, 1, 14),
            Err(TyperError::SourceFunctionClassNotFound {
                kind: SourceFunctionKind::Ordinary,
                arity: 1
            })
        ));
    }
}
