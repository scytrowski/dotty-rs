//! Milestone 5d2a: constructor completion, on real Scala 3.9.0 fixtures (see
//! `fixtures/semantic/Constructors.scala`).
//!
//! Addresses used below (absolute AST addresses, checked against the actual
//! node tag before use so a regenerated fixture fails loudly instead of
//! testing something else):
//!
//! | fixture | class | ctor | notes |
//! |---------|-------|------|-------|
//! | `CtorEmpty` | 4 | 21 | no parameters |
//! | `CtorPlain` | 4 | 30 | `(x: Int)` |
//! | `CtorWithVal` | 4 | 28 | `(val x: Int)`; header field PARAM at 9, ctor's own PARAM at 31 |
//! | `CtorGeneric` | 4 | 48 | `[A](x: A)`; header TYPEPARAM at 9, ctor's own at 51 |
//! | `CtorGenericContextOnly` | 4 | 55 | `[A](using ord: Ordering[A])` |
//! | `CtorContextOnly` | 4 | 37 | `(using ord: Ordering[Int])` |
//! | `CtorOldImplicit` | 4 | 37 | `(implicit ord: Ordering[Int])` |
//! | `CtorCurriedGeneric` | 4 | 62 | `[A](x: A)(using ord: Ordering[A])` |
//! | `CtorTwoTermClauses` | 4 | 44 | `(x: Int)(using y: Ordering[Int])` |
//! | `CtorObj` | term 4, module class 24 | 48 | `object CtorObj` |
//! | `CtorOuter` / `CtorInner` | 4 / 30 | 21 / 75 | `Outer: class Inner[A](x: A)` |

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::types::{MethodKind, Type};
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{StandardSection, TYPEDEF_TAG, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

macro_rules! fixture {
    ($name:ident, $file:literal) => {
        const $name: &[u8] = include_bytes!(concat!("fixtures/semantic/", $file));
    };
}

fixture!(CTOR_EMPTY, "CtorEmpty.tasty");
fixture!(CTOR_PLAIN, "CtorPlain.tasty");
fixture!(CTOR_WITH_VAL, "CtorWithVal.tasty");
fixture!(CTOR_GENERIC, "CtorGeneric.tasty");
fixture!(CTOR_GENERIC_CONTEXT_ONLY, "CtorGenericContextOnly.tasty");
fixture!(CTOR_CONTEXT_ONLY, "CtorContextOnly.tasty");
fixture!(CTOR_OLD_IMPLICIT, "CtorOldImplicit.tasty");
fixture!(CTOR_CURRIED_GENERIC, "CtorCurriedGeneric.tasty");
fixture!(CTOR_TWO_TERM_CLAUSES, "CtorTwoTermClauses.tasty");
fixture!(CTOR_OBJ, "CtorObj.tasty");
fixture!(CTOR_OUTER, "CtorOuter.tasty");
fixture!(CTOR_ORD, "CtorOrd.tasty");

struct Session {
    store: SemanticStore,
    definitions: Definitions,
}

impl Session {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        Self { store, definitions }
    }

    fn text(&self, name: dotty_core::ids::NameId) -> &str {
        self.store.names.resolve(name)
    }

    /// The symbol a `TypeRef(no_prefix, symbol)` names, or `None` for any
    /// other shape (including a `TypeRef` with any other prefix).
    fn owner_ref(&self, ty: TypeId) -> Option<SymbolId> {
        match self.store.types.get(ty) {
            Type::TypeRef { prefix, target } if *prefix == self.definitions.no_prefix => {
                target.symbol()
            }
            _ => None,
        }
    }
}

/// Stub classes for `scala.Int`, `scala.AnyRef`, `scala.Unit`,
/// `scala.Nothing` and `scala.Any`, the only external names the implicit
/// `AnyRef` parent, the primitive parameter types and the serialized (and
/// discarded) constructor return tree reach. `CtorOrd`, the fixtures'
/// contextual parameter type, is entered as a real unit instead (see
/// [`entered`]), not stubbed.
fn stub_classes(session: &mut Session, packages: &mut Packages, path: &[&str], names: &[&str]) {
    use dotty_core::{Symbol, SymbolFlags, SymbolLinks, Visibility};
    let package = packages
        .enter(&mut session.store, SymbolOrigin::Synthetic, path)
        .pop()
        .unwrap();
    for class in names {
        let name = Name::new(session.store.names.intern(class), Namespace::Type);
        let symbol = session.store.symbols.alloc(Symbol {
            name,
            owner: Some(package.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        session
            .store
            .scopes
            .get_mut(package.scope)
            .enter(name, symbol);
    }
}

/// Enters `CtorOrd.tasty` (a real unit, sharing the registry that follows)
/// and stubs the primitive classes, then enters `file` against that shared
/// registry.
fn entered<'a>(file: &'a TastyFile<'a>, session: &'a mut Session) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    stub_classes(
        session,
        &mut packages,
        &["scala"],
        &["Int", "AnyRef", "Unit", "Nothing", "Any"],
    );
    stub_classes(session, &mut packages, &["java", "lang"], &["Object"]);
    let ord_file = TastyFile::parse_scala_3_9(CTOR_ORD).unwrap();
    // `enter_symbols` only needs `&mut session.store`, so this doesn't keep
    // `ord_file` (a local) borrowed past this call.
    let (_, packages) = {
        let mut ord_unpickler = TastyUnpickler::with_packages(
            &ord_file,
            &mut session.store,
            session.definitions,
            packages,
        );
        ord_unpickler.enter_symbols().unwrap();
        ord_unpickler.into_parts()
    };

    let mut unpickler =
        TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    unpickler
}

/// Asserts `at` is a `TYPEDEF` (so a regenerated fixture fails loudly instead
/// of testing something else) and returns its entered symbol.
fn checked_class(
    unpickler: &TastyUnpickler<'_, '_, '_>,
    file: &TastyFile<'_>,
    at: u32,
) -> SymbolId {
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(payload[at as usize], TYPEDEF_TAG, "not a TYPEDEF at {at}");
    unpickler.index().symbol_at(at).expect("entered symbol")
}

fn method(store: &SemanticStore, ty: TypeId) -> &dotty_core::types::MethodType {
    match store.types.get(ty) {
        Type::Method(method) => method,
        other => panic!("not a method: {other:?}"),
    }
}

fn poly(store: &SemanticStore, ty: TypeId) -> &dotty_core::types::PolyType {
    match store.types.get(ty) {
        Type::Poly(poly) => poly,
        other => panic!("not a poly: {other:?}"),
    }
}

#[test]
fn a_no_arg_constructor_completes_to_an_empty_method_constructing_its_owner() {
    let file = TastyFile::parse_scala_3_9(CTOR_EMPTY).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(21).unwrap();
    let class_state = unpickler.symbol_state_at(4);
    drop(unpickler);

    let m = method(&session.store, ty);
    assert!(m.params.is_empty());
    assert_eq!(m.kind, MethodKind::Plain);
    assert_eq!(session.owner_ref(m.result), Some(class));
    // Constructor completion never forces its owner's ClassInfo.
    assert_eq!(class_state, Some((SymbolKind::Class, SymbolInfo::Missing)));
}

#[test]
fn completing_a_constructor_twice_returns_the_same_type_and_allocates_nothing() {
    let file = TastyFile::parse_scala_3_9(CTOR_EMPTY).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let first = unpickler.complete_symbol(21).unwrap();
    let types = unpickler.index().type_count();
    let second = unpickler.complete_symbol(21).unwrap();

    assert_eq!(first, second);
    assert_eq!(unpickler.index().type_count(), types);
}

#[test]
fn an_ordinary_term_clause_is_unchanged_and_carries_its_parameter() {
    let file = TastyFile::parse_scala_3_9(CTOR_PLAIN).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(30).unwrap();
    drop(unpickler);

    let m = method(&session.store, ty);
    assert_eq!(m.params.len(), 1);
    assert_eq!(session.text(m.params[0].name.as_name().text()), "x");
    assert!(!m.params[0].erased && !m.params[0].varargs);
    assert_eq!(m.kind, MethodKind::Plain);
    assert_eq!(session.owner_ref(m.result), Some(class));
}

#[test]
fn a_class_header_field_and_the_constructors_own_parameter_are_distinct_symbols() {
    // `class CtorWithVal(val x: Int)`: the header PARAM at 9 (the field) and
    // the constructor DEFDEF's own PARAM at 31 are different definitions.
    let file = TastyFile::parse_scala_3_9(CTOR_WITH_VAL).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let field = unpickler.index().symbol_at(9).expect("the field symbol");
    let ctor_param_symbol = unpickler.index().symbol_at(31).expect("the ctor's own x");
    let ty = unpickler.complete_symbol(28).unwrap();
    drop(unpickler);

    assert_ne!(ctor_param_symbol, field);
    let m = method(&session.store, ty);
    assert_eq!(m.params.len(), 1);
    assert_eq!(session.owner_ref(m.result), Some(class));
}

#[test]
fn a_generic_constructor_abstracts_its_own_type_parameter_into_the_result() {
    // `class CtorGeneric[A](x: A)`: the class header's `A` (address 9) and the
    // constructor's own leading type clause `A` (address 51) are distinct
    // symbols; only the constructor's own is what the Poly abstracts.
    let file = TastyFile::parse_scala_3_9(CTOR_GENERIC).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let header_a = unpickler.index().symbol_at(9).expect("the class header A");
    let ctor_a = unpickler.index().symbol_at(51).expect("the ctor's own A");
    let ty = unpickler.complete_symbol(48).unwrap();
    drop(unpickler);

    assert_ne!(header_a, ctor_a);
    let p = poly(&session.store, ty);
    assert_eq!(p.params.len(), 1);
    let m = method(&session.store, p.result);
    assert_eq!(m.params.len(), 1);
    // The term parameter's type is `ParamRef(poly, 0)`.
    assert_eq!(
        session.store.types.get(m.params[0].ty),
        &Type::ParamRef {
            binder: ty,
            index: 0
        }
    );
    // The result is `CtorGeneric[ParamRef(poly, 0)]`.
    let Type::Applied { tycon, args } = session.store.types.get(m.result) else {
        panic!("not an applied type");
    };
    assert_eq!(session.owner_ref(*tycon), Some(class));
    assert_eq!(args.len(), 1);
    assert_eq!(
        session.store.types.get(args[0]),
        &Type::ParamRef {
            binder: ty,
            index: 0
        }
    );
}

#[test]
fn a_leading_implicit_term_clause_gets_a_synthetic_empty_ordinary_clause_first() {
    // `class CtorOldImplicit(implicit ord: Ordering[Int])` normalizes to
    // `Method() -> ImplicitMethod(ord) -> C`.
    let file = TastyFile::parse_scala_3_9(CTOR_OLD_IMPLICIT).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(37).unwrap();
    drop(unpickler);

    let outer = method(&session.store, ty);
    assert!(outer.params.is_empty());
    assert_eq!(outer.kind, MethodKind::Plain);
    let inner = method(&session.store, outer.result);
    assert_eq!(inner.params.len(), 1);
    assert_eq!(inner.kind, MethodKind::Implicit);
    assert_eq!(session.text(inner.params[0].name.as_name().text()), "ord");
    assert_eq!(session.owner_ref(inner.result), Some(class));
}

#[test]
fn a_contextual_only_constructor_gets_a_synthetic_empty_ordinary_clause_after() {
    // `class CtorContextOnly(using ord: Ordering[Int])` normalizes to
    // `ContextualMethod(ord) -> Method() -> C`.
    let file = TastyFile::parse_scala_3_9(CTOR_CONTEXT_ONLY).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(37).unwrap();
    drop(unpickler);

    let outer = method(&session.store, ty);
    assert_eq!(outer.params.len(), 1);
    assert_eq!(outer.kind, MethodKind::Contextual);
    assert_eq!(session.text(outer.params[0].name.as_name().text()), "ord");
    let inner = method(&session.store, outer.result);
    assert!(inner.params.is_empty());
    assert_eq!(inner.kind, MethodKind::Plain);
    assert_eq!(session.owner_ref(inner.result), Some(class));
}

#[test]
fn type_params_and_a_contextual_only_constructor_keep_the_type_clause_leading() {
    // `class CtorGenericContextOnly[A](using ord: Ordering[A])` normalizes to
    // `Poly[A] -> ContextualMethod(ord) -> Method() -> C[A]`.
    let file = TastyFile::parse_scala_3_9(CTOR_GENERIC_CONTEXT_ONLY).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(55).unwrap();
    drop(unpickler);

    let p = poly(&session.store, ty);
    assert_eq!(p.params.len(), 1);
    let contextual = method(&session.store, p.result);
    assert_eq!(contextual.kind, MethodKind::Contextual);
    assert_eq!(contextual.params.len(), 1);
    let plain = method(&session.store, contextual.result);
    assert!(plain.params.is_empty());
    assert_eq!(plain.kind, MethodKind::Plain);
    let Type::Applied { tycon, args } = session.store.types.get(plain.result) else {
        panic!("not applied");
    };
    assert_eq!(session.owner_ref(*tycon), Some(class));
    assert_eq!(args.len(), 1);
    assert_eq!(
        session.store.types.get(args[0]),
        &Type::ParamRef {
            binder: ty,
            index: 0
        }
    );
}

#[test]
fn a_curried_constructor_with_an_ordinary_clause_before_using_is_unchanged() {
    // `class CtorCurriedGeneric[A](x: A)(using ord: Ordering[A])`: no
    // synthetic clause, because the first term clause is already ordinary.
    let file = TastyFile::parse_scala_3_9(CTOR_CURRIED_GENERIC).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(62).unwrap();
    drop(unpickler);

    let p = poly(&session.store, ty);
    let plain = method(&session.store, p.result);
    assert_eq!(plain.kind, MethodKind::Plain);
    assert_eq!(plain.params.len(), 1);
    let contextual = method(&session.store, plain.result);
    assert_eq!(contextual.kind, MethodKind::Contextual);
    assert_eq!(contextual.params.len(), 1);
    // The leading type clause makes the result `CtorCurriedGeneric[ParamRef]`.
    let Type::Applied { tycon, args } = session.store.types.get(contextual.result) else {
        panic!("not applied");
    };
    assert_eq!(session.owner_ref(*tycon), Some(class));
    assert_eq!(args.len(), 1);
    assert_eq!(
        session.store.types.get(args[0]),
        &Type::ParamRef {
            binder: ty,
            index: 0
        }
    );
}

#[test]
fn an_ordinary_clause_followed_by_a_using_clause_is_unchanged() {
    let file = TastyFile::parse_scala_3_9(CTOR_TWO_TERM_CLAUSES).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let class = checked_class(&unpickler, &file, 4);
    let ty = unpickler.complete_symbol(44).unwrap();
    drop(unpickler);

    let plain = method(&session.store, ty);
    assert_eq!(plain.kind, MethodKind::Plain);
    assert_eq!(plain.params.len(), 1);
    let contextual = method(&session.store, plain.result);
    assert_eq!(contextual.kind, MethodKind::Contextual);
    assert_eq!(contextual.params.len(), 1);
    assert_eq!(session.owner_ref(contextual.result), Some(class));
}

#[test]
fn an_objects_constructor_is_owned_by_the_module_class_not_the_object_term() {
    let file = TastyFile::parse_scala_3_9(CTOR_OBJ).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let module_class = checked_class(&unpickler, &file, 24);
    let object_term = unpickler.index().symbol_at(4).unwrap();
    assert_ne!(module_class, object_term);
    let ty = unpickler.complete_symbol(48).unwrap();
    drop(unpickler);

    let m = method(&session.store, ty);
    assert!(m.params.is_empty());
    assert_eq!(
        session.owner_ref(m.result),
        Some(module_class),
        "the constructor's result must name the ModuleClass, never the Object term symbol"
    );
}

#[test]
fn a_nested_class_constructors_result_names_only_the_inner_class() {
    // `class CtorOuter: class CtorInner[A](x: A)`: the repository's `no_prefix`
    // convention holds even nested, with no `Outer.this` injected.
    let file = TastyFile::parse_scala_3_9(CTOR_OUTER).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);
    let outer = checked_class(&unpickler, &file, 4);
    let inner = checked_class(&unpickler, &file, 30);
    assert_ne!(outer, inner);
    let ty = unpickler.complete_symbol(75).unwrap();
    drop(unpickler);

    let p = poly(&session.store, ty);
    let m = method(&session.store, p.result);
    let Type::Applied { tycon, .. } = session.store.types.get(m.result) else {
        panic!("not applied");
    };
    let Type::TypeRef { prefix, target } = session.store.types.get(*tycon) else {
        panic!("not a type ref");
    };
    assert_eq!(*prefix, session.definitions.no_prefix);
    assert_eq!(target.symbol(), Some(inner));
    assert_ne!(target.symbol(), Some(outer));
}

#[test]
fn a_constructor_completes_while_its_owner_class_stays_missing() {
    let file = TastyFile::parse_scala_3_9(CTOR_PLAIN).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    let before = unpickler.symbol_state_at(4);
    assert!(unpickler.complete_symbol(30).is_ok());
    let after = unpickler.symbol_state_at(4);

    assert_eq!(before, Some((SymbolKind::Class, SymbolInfo::Missing)));
    assert_eq!(
        after,
        Some((SymbolKind::Class, SymbolInfo::Missing)),
        "constructor completion must not force class completion"
    );
}

#[test]
fn a_constructor_completes_while_its_owner_class_is_already_complete() {
    // `CtorEmpty` extends the implicit `AnyRef`/`Object` parent, so its class
    // completes with the same stubs the constructor needs.
    let file = TastyFile::parse_scala_3_9(CTOR_EMPTY).unwrap();
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session);

    unpickler.complete_symbol(4).expect("the class completes");
    let class_state = unpickler.symbol_state_at(4);
    let ty = unpickler.complete_symbol(21).unwrap();
    drop(unpickler);

    assert!(matches!(
        class_state,
        Some((SymbolKind::Class, SymbolInfo::Complete(_)))
    ));
    let m = method(&session.store, ty);
    assert!(m.params.is_empty());
}

#[test]
fn a_failure_completing_a_parameter_leaves_the_constructor_missing_and_undoes_it() {
    // Without the `Int` stub, `CtorPlain`'s parameter type is unresolved.
    let file = TastyFile::parse_scala_3_9(CTOR_PLAIN).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    unpickler.enter_symbols().unwrap();
    let types = unpickler.index().type_count();

    let result = unpickler.complete_symbol(30);

    assert!(
        matches!(
            result,
            Err(UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. })
        ),
        "{result:?}"
    );
    assert_eq!(
        unpickler.symbol_state_at(30),
        Some((SymbolKind::Constructor, SymbolInfo::Missing))
    );
    assert_eq!(unpickler.index().type_count(), types);
    // A retry fails the same way.
    assert_eq!(unpickler.complete_symbol(30), result);
}
