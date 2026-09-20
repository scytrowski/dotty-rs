//! Semantic fixtures from `docs/dotty-core-design.md` §14.
//!
//! Each fixture builds, **by hand** (no parser exists yet), the
//! `SemanticStore` state a namer/typer would eventually produce for a small
//! named Scala fragment, and asserts its shape. This both documents the
//! intended shape and pins it before any namer exists to produce it
//! automatically.
//!
//! A recurring simplification: several fixtures need a type standing in for
//! an unconstrained bound (`Nothing..Any`, or a type constructor's `Any`
//! upper bound) or an external library type (`Show`, `Either`, `Iterable`)
//! that this foundation has no builtin symbol for. Each such placeholder is
//! a freshly allocated `Type::NoPrefix` or a `TypeRef` to a synthetic
//! symbol, exactly as a real classfile/TASTy adapter would allocate one for
//! an external reference — the fixtures only assert the *shape* that
//! matters for the construct under test, not full standard-library fidelity.

use dotty_core::{
    Annotation, ClassInfo, MatchType, MethodKind, MethodParam, MethodType, PolyType, Scope,
    SemanticStore, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin,
    TermName, Type, TypeLambda, TypeName, TypeParam, Visibility,
};

/// Allocates a fresh, semantically-opaque placeholder type (standing in for
/// an unconstrained bound or an unmodeled external type).
fn placeholder(store: &mut SemanticStore) -> dotty_core::TypeId {
    store.types.alloc(Type::NoPrefix)
}

fn synthetic_symbol(
    store: &mut SemanticStore,
    name: dotty_core::Name,
    kind: SymbolKind,
) -> dotty_core::SymbolId {
    store.symbols.alloc(Symbol {
        name,
        owner: None,
        kind,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    })
}

/// `def identity[A](x: A): A = x`
#[test]
fn generic_identity_method() {
    let mut store = SemanticStore::new();

    let a_name = TypeName::new(store.names.intern("A"));
    let x_name = TermName::new(store.names.intern("x"));
    let identity_name = TermName::new(store.names.intern("identity"));

    let poly_binder = store.types.reserve();
    let param_ref = store.types.alloc(Type::ParamRef {
        binder: poly_binder.id(),
        index: 0,
    });
    let method = store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: x_name,
            ty: param_ref,
            erased: false,
            varargs: false,
        }],
        result: param_ref,
        kind: MethodKind::Plain,
    }));
    let bounds = placeholder(&mut store);
    let type_param_a = TypeParam {
        name: a_name,
        bounds,
        declared_variance: None,
    };
    let poly = store.types.fill(
        poly_binder,
        Type::Poly(PolyType {
            params: vec![type_param_a],
            result: method,
        }),
    );

    let symbol = synthetic_symbol(&mut store, *identity_name.as_name(), SymbolKind::Method);
    store.symbols.set_info(symbol, SymbolInfo::Complete(poly));

    let Type::Poly(resolved) = store.types.get(poly) else {
        panic!("expected a Poly type");
    };
    assert_eq!(resolved.params, vec![type_param_a]);
    let Type::Method(resolved_method) = store.types.get(resolved.result) else {
        panic!("expected the Poly's result to be a Method");
    };
    let &Type::ParamRef { binder, index } = store.types.get(resolved_method.result) else {
        panic!("expected the method's result to be a ParamRef");
    };
    assert_eq!(binder, poly);
    assert_eq!(index, 0);
}

/// `def foo(x: Int): Int` / `def foo(x: String): String`, both visible under
/// the same name in one scope.
#[test]
fn overloaded_methods_share_a_scope_entry() {
    let mut store = SemanticStore::new();

    let foo_name = TermName::new(store.names.intern("foo"));
    let int_overload = synthetic_symbol(&mut store, *foo_name.as_name(), SymbolKind::Method);
    let string_overload = synthetic_symbol(&mut store, *foo_name.as_name(), SymbolKind::Method);

    let scope_id = store.scopes.alloc(Scope::new(None));
    let scope = store.scopes.get_mut(scope_id);
    scope.enter(*foo_name.as_name(), int_overload);
    scope.enter(*foo_name.as_name(), string_overload);

    let scope = store.scopes.get(scope_id);
    assert_eq!(
        scope.lookup_all(foo_name.as_name()),
        &[int_overload, string_overload]
    );
    assert_eq!(scope.lookup(foo_name.as_name()), Some(int_overload));
}

/// `class Foo` / `object Foo`, linked as companions.
#[test]
fn class_and_companion_object() {
    let mut store = SemanticStore::new();
    let foo_name_id = store.names.intern("Foo");
    let class_name = TypeName::new(foo_name_id);
    let object_name = TermName::new(foo_name_id);

    let class_symbol = synthetic_symbol(&mut store, *class_name.as_name(), SymbolKind::Class);
    let object_symbol = synthetic_symbol(&mut store, *object_name.as_name(), SymbolKind::Object);

    store.symbols.get_mut(class_symbol).links.companion = Some(object_symbol);
    store.symbols.get_mut(object_symbol).links.companion = Some(class_symbol);

    let declarations = store.scopes.alloc(Scope::new(Some(class_symbol)));
    let no_prefix = placeholder(&mut store);
    let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix: no_prefix,
        class: class_symbol,
        parents: Vec::new(),
        declarations,
        self_type: None,
    }));
    store
        .symbols
        .set_info(class_symbol, SymbolInfo::Complete(class_info));

    assert_eq!(
        store.symbols.get(class_symbol).links.companion,
        Some(object_symbol)
    );
    assert_eq!(
        store.symbols.get(object_symbol).links.companion,
        Some(class_symbol)
    );
    let SymbolInfo::Complete(resolved_info) = store.symbols.get(class_symbol).info else {
        panic!("expected the class symbol to have a complete ClassInfo");
    };
    let Type::ClassInfo(resolved) = store.types.get(resolved_info) else {
        panic!("expected a ClassInfo type");
    };
    assert_eq!(resolved.declarations, declarations);
}

/// `trait Foo { type T }` and `def f(x: Foo): x.T`: a path-dependent type.
#[test]
fn path_dependent_type() {
    let mut store = SemanticStore::new();

    let foo_name = TypeName::new(store.names.intern("Foo"));
    let t_name = TypeName::new(store.names.intern("T"));
    let x_name = TermName::new(store.names.intern("x"));

    let trait_symbol = synthetic_symbol(&mut store, *foo_name.as_name(), SymbolKind::Trait);
    let t_symbol = synthetic_symbol(&mut store, *t_name.as_name(), SymbolKind::TypeAlias);
    store.symbols.get_mut(t_symbol).owner = Some(trait_symbol);

    let declarations = store.scopes.alloc(Scope::new(Some(trait_symbol)));
    store
        .scopes
        .get_mut(declarations)
        .enter(*t_name.as_name(), t_symbol);

    let no_prefix = placeholder(&mut store);
    let trait_info = store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix: no_prefix,
        class: trait_symbol,
        parents: Vec::new(),
        declarations,
        self_type: None,
    }));
    store
        .symbols
        .set_info(trait_symbol, SymbolInfo::Complete(trait_info));

    // The parameter `x: Foo`.
    let x_symbol = synthetic_symbol(&mut store, *x_name.as_name(), SymbolKind::Parameter);
    let foo_type_ref = store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        symbol: trait_symbol,
    });
    store
        .symbols
        .set_info(x_symbol, SymbolInfo::Complete(foo_type_ref));

    // `x.T`: a TypeRef whose prefix is `x`'s own singleton (TermRef) type.
    let x_singleton = store.types.alloc(Type::TermRef {
        prefix: no_prefix,
        symbol: x_symbol,
    });
    let path_dependent = store.types.alloc(Type::TypeRef {
        prefix: x_singleton,
        symbol: t_symbol,
    });

    let Type::TypeRef { prefix, symbol } = *store.types.get(path_dependent) else {
        panic!("expected a TypeRef");
    };
    assert_eq!(symbol, t_symbol);
    let Type::TermRef {
        symbol: prefix_symbol,
        ..
    } = *store.types.get(prefix)
    else {
        panic!("expected the prefix to be x's singleton TermRef");
    };
    assert_eq!(prefix_symbol, x_symbol);
    assert_eq!(store.symbols.get(t_symbol).owner, Some(trait_symbol));
}

/// `trait Functor[F[_]]`: `F`'s bounds are themselves a type lambda,
/// capturing that `F` ranges over unary type constructors.
#[test]
fn higher_kinded_type_parameter() {
    let mut store = SemanticStore::new();

    let f_name = TypeName::new(store.names.intern("F"));
    let underscore_name = TypeName::new(store.names.intern("_"));

    // `[_] =>> Any`: the kind of an unconstrained unary type constructor.
    let hk_binder = store.types.reserve();
    let wildcard_bounds = placeholder(&mut store);
    let wildcard_param = TypeParam {
        name: underscore_name,
        bounds: wildcard_bounds,
        declared_variance: None,
    };
    let hk_body = placeholder(&mut store);
    let hk_kind = store.types.fill(
        hk_binder,
        Type::TypeLambda(TypeLambda {
            params: vec![wildcard_param],
            result: hk_body,
        }),
    );

    let functor_poly_binder = store.types.reserve();
    let f_type_param = TypeParam {
        name: f_name,
        bounds: hk_kind,
        declared_variance: None,
    };
    let functor_body = placeholder(&mut store);
    let functor_poly = store.types.fill(
        functor_poly_binder,
        Type::Poly(PolyType {
            params: vec![f_type_param],
            result: functor_body,
        }),
    );

    let Type::Poly(resolved) = store.types.get(functor_poly) else {
        panic!("expected a Poly type");
    };
    assert_eq!(resolved.params.len(), 1);
    let Type::TypeLambda(resolved_kind) = store.types.get(resolved.params[0].bounds) else {
        panic!("expected F's bounds to be a TypeLambda");
    };
    assert_eq!(resolved_kind.params.len(), 1);
}

/// `def show[A](x: A)(using Show[A]): String`: a contextual parameter
/// clause.
#[test]
fn contextual_parameter() {
    let mut store = SemanticStore::new();

    let a_name = TypeName::new(store.names.intern("A"));
    let x_name = TermName::new(store.names.intern("x"));
    let given_name = TermName::new(store.names.intern("$given"));
    let show_name = TypeName::new(store.names.intern("Show"));

    let poly_binder = store.types.reserve();
    let param_ref = store.types.alloc(Type::ParamRef {
        binder: poly_binder.id(),
        index: 0,
    });

    let show_symbol = synthetic_symbol(&mut store, *show_name.as_name(), SymbolKind::Trait);
    let no_prefix = placeholder(&mut store);
    let show_tycon = store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        symbol: show_symbol,
    });
    let show_of_a = store.types.alloc(Type::Applied {
        tycon: show_tycon,
        args: vec![param_ref],
    });

    let string_result = placeholder(&mut store);
    let contextual_method = store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: given_name,
            ty: show_of_a,
            erased: false,
            varargs: false,
        }],
        result: string_result,
        kind: MethodKind::Contextual,
    }));

    let plain_method = store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: x_name,
            ty: param_ref,
            erased: false,
            varargs: false,
        }],
        result: contextual_method,
        kind: MethodKind::Plain,
    }));

    let bounds = placeholder(&mut store);
    let poly = store.types.fill(
        poly_binder,
        Type::Poly(PolyType {
            params: vec![TypeParam {
                name: a_name,
                bounds,
                declared_variance: None,
            }],
            result: plain_method,
        }),
    );

    let Type::Poly(resolved_poly) = store.types.get(poly) else {
        panic!("expected a Poly type");
    };
    let Type::Method(outer) = store.types.get(resolved_poly.result) else {
        panic!("expected the Poly's result to be a Method");
    };
    assert_eq!(outer.kind, MethodKind::Plain);
    let Type::Method(inner) = store.types.get(outer.result) else {
        panic!("expected the outer method's result to be another Method");
    };
    assert_eq!(inner.kind, MethodKind::Contextual);
    let Type::Applied { tycon, args } = store.types.get(inner.params[0].ty) else {
        panic!("expected the contextual parameter's type to be Applied");
    };
    assert_eq!(*tycon, show_tycon);
    assert_eq!(*args, vec![param_ref]);
}

/// `[X] =>> Either[String, X]`.
#[test]
fn type_lambda() {
    let mut store = SemanticStore::new();

    let x_name = TypeName::new(store.names.intern("X"));
    let either_name = TypeName::new(store.names.intern("Either"));

    let lambda_binder = store.types.reserve();
    let x_param_ref = store.types.alloc(Type::ParamRef {
        binder: lambda_binder.id(),
        index: 0,
    });

    let either_symbol = synthetic_symbol(&mut store, *either_name.as_name(), SymbolKind::Class);
    let no_prefix = placeholder(&mut store);
    let either_tycon = store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        symbol: either_symbol,
    });
    let string_arg = placeholder(&mut store);
    let either_applied = store.types.alloc(Type::Applied {
        tycon: either_tycon,
        args: vec![string_arg, x_param_ref],
    });

    let x_bounds = placeholder(&mut store);
    let lambda = store.types.fill(
        lambda_binder,
        Type::TypeLambda(TypeLambda {
            params: vec![TypeParam {
                name: x_name,
                bounds: x_bounds,
                declared_variance: None,
            }],
            result: either_applied,
        }),
    );

    let Type::TypeLambda(resolved) = store.types.get(lambda) else {
        panic!("expected a TypeLambda");
    };
    let Type::Applied { args, .. } = store.types.get(resolved.result) else {
        panic!("expected the lambda's result to be Applied");
    };
    let &Type::ParamRef { binder, index } = store.types.get(args[1]) else {
        panic!("expected the second argument to be a ParamRef");
    };
    assert_eq!(binder, lambda);
    assert_eq!(index, 0);
}

/// `type Elem[X] = X match { case Iterable[t] => t }`.
///
/// The pattern's own bound variable `t` is modeled as a fresh, independent
/// type reference rather than through the binder machinery used for method/
/// poly/lambda parameters: `Type::MatchCase` has no `params` field of its
/// own in this foundation, so per-case pattern variables are out of scope
/// for now (see `docs/dotty-core-design.md` §15's "not in scope" list,
/// which already excludes full match-type semantics).
#[test]
fn match_type() {
    let mut store = SemanticStore::new();

    let x_name = TypeName::new(store.names.intern("X"));
    let iterable_name = TypeName::new(store.names.intern("Iterable"));

    let lambda_binder = store.types.reserve();
    let x_param_ref = store.types.alloc(Type::ParamRef {
        binder: lambda_binder.id(),
        index: 0,
    });

    let iterable_symbol = synthetic_symbol(&mut store, *iterable_name.as_name(), SymbolKind::Class);
    let no_prefix = placeholder(&mut store);
    let iterable_tycon = store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        symbol: iterable_symbol,
    });
    let t_placeholder = placeholder(&mut store);
    let pattern = store.types.alloc(Type::Applied {
        tycon: iterable_tycon,
        args: vec![t_placeholder],
    });
    let case = store.types.alloc(Type::MatchCase {
        pattern,
        result: t_placeholder,
    });

    let bound = placeholder(&mut store);
    let match_type = store.types.alloc(Type::Match(MatchType {
        bound,
        scrutinee: x_param_ref,
        cases: vec![case],
    }));

    let x_bounds = placeholder(&mut store);
    let elem_lambda = store.types.fill(
        lambda_binder,
        Type::TypeLambda(TypeLambda {
            params: vec![TypeParam {
                name: x_name,
                bounds: x_bounds,
                declared_variance: None,
            }],
            result: match_type,
        }),
    );

    let Type::TypeLambda(resolved) = store.types.get(elem_lambda) else {
        panic!("expected a TypeLambda");
    };
    let Type::Match(resolved_match) = store.types.get(resolved.result) else {
        panic!("expected the lambda's result to be a Match type");
    };
    assert_eq!(resolved_match.scrutinee, x_param_ref);
    assert_eq!(resolved_match.cases, vec![case]);
    let Type::MatchCase {
        pattern: resolved_pattern,
        result: resolved_result,
    } = store.types.get(case)
    else {
        panic!("expected a MatchCase");
    };
    assert_eq!(*resolved_pattern, pattern);
    assert_eq!(*resolved_result, t_placeholder);
}

/// `@deprecated class Foo`: a semantic annotation on a symbol.
#[test]
fn annotated_symbol() {
    let mut store = SemanticStore::new();

    let deprecated_name = TypeName::new(store.names.intern("deprecated"));
    let foo_name = TypeName::new(store.names.intern("Foo"));

    let deprecated_symbol =
        synthetic_symbol(&mut store, *deprecated_name.as_name(), SymbolKind::Class);
    let no_prefix = placeholder(&mut store);
    let deprecated_type = store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        symbol: deprecated_symbol,
    });
    let annotation_id = store
        .annotations
        .alloc(Annotation::new(deprecated_type, None));

    let foo_symbol = synthetic_symbol(&mut store, *foo_name.as_name(), SymbolKind::Class);
    store
        .symbols
        .get_mut(foo_symbol)
        .annotations
        .push(annotation_id);

    assert_eq!(
        store.symbols.get(foo_symbol).annotations,
        vec![annotation_id]
    );
    let resolved = store.annotations.get(annotation_id);
    assert_eq!(resolved.ty, deprecated_type);
    assert_eq!(resolved.tree, None);
}
