//! Canonical identity lookup for source-level Scala function classes.

use super::*;

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

/// Largest source function arity materialized by this typer increment.
///
/// Scala's compiler can synthesize function classes above arity 22 and erases
/// them through `FunctionXXL`; this source typer does not synthesize those
/// classes and therefore reports such requests explicitly.
pub const MAX_SOURCE_FUNCTION_ARITY: usize = 22;

impl SourceTyper<'_> {
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
                self.resolver
                    .resolve_member(self.store, &request)
                    .map_err(|error| TyperError::SymbolResolution {
                        source: self.source,
                        tree_index,
                        error,
                    })?
                    .ok_or(TyperError::SourceFunctionClassNotFound { kind, arity })?
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
            Ok((path == ["scala"]).then_some(self.package).flatten())
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
    fn missing_classpath_function_class_is_reported_and_failed_resolution_rolls_back() {
        let (arena, index, mut store, packages, definitions, scala) = setup();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let resolver = FunctionResolver {
            package: Some(scala),
            requests: Rc::clone(&requests),
            ..FunctionResolver::default()
        };
        let mut typer = typer(&arena, &index, &mut store, &packages, definitions, resolver);

        assert!(matches!(
            typer.source_function_class(SourceFunctionKind::Contextual, 3, 12),
            Err(TyperError::SourceFunctionClassNotFound {
                kind: SourceFunctionKind::Contextual,
                arity: 3
            })
        ));
        assert!(requests.borrow().is_empty());
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
