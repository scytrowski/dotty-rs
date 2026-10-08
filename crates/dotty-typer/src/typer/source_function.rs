//! Canonical identity lookup for source-level Scala function classes.

use super::*;
use dotty_core::{
    ClassInfo, MethodKind, MethodParam, Namespace, Scope, Symbol, SymbolFlags, SymbolInfo,
    SymbolKind, SymbolOrigin, TermName, Visibility,
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
