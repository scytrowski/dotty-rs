//! Milestone 5d1: completing `Class`, `Trait` and `ModuleClass` symbols to
//! `Complete(Type::ClassInfo)`, on the real Scala 3.9.0 `ClassInfos.scala`
//! fixtures. Each top-level definition is its own unit; the units are entered
//! into one session that shares the store and the package registry.
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::types::{ClassInfo, Type};
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{DefinitionBody, StructuredNode, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const BASE: &[u8] = include_bytes!("fixtures/semantic/InfoBase.tasty");
const DEP: &[u8] = include_bytes!("fixtures/semantic/InfoDep.tasty");
const PARENT: &[u8] = include_bytes!("fixtures/semantic/InfoParent.tasty");
const CHILD: &[u8] = include_bytes!("fixtures/semantic/InfoChild.tasty");
const HOLDER: &[u8] = include_bytes!("fixtures/semantic/InfoHolder.tasty");
const WRAPPED: &[u8] = include_bytes!("fixtures/semantic/InfoWrapped.tasty");
const LAM: &[u8] = include_bytes!("fixtures/semantic/InfoLam.tasty");
const LAMBDA_PARENT: &[u8] = include_bytes!("fixtures/semantic/InfoLambdaParent.tasty");
const LAMBDA_SELF: &[u8] = include_bytes!("fixtures/semantic/InfoLambdaSelf.tasty");
const GEN_SELF: &[u8] = include_bytes!("fixtures/semantic/InfoGenSelf.tasty");
const NEEDS_GENERIC: &[u8] = include_bytes!("fixtures/semantic/InfoNeedsGeneric.tasty");
const NEEDS: &[u8] = include_bytes!("fixtures/semantic/InfoNeeds.tasty");

/// A session: the shared store and packages, and the entered units.
struct Session {
    store: SemanticStore,
    definitions: Definitions,
    packages: Option<Packages>,
}

impl Session {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        for (path, names) in [
            (&["scala"][..], &["Int", "Any", "Nothing"][..]),
            (&["java", "lang"][..], &["Object"][..]),
        ] {
            stub_classes(&mut store, &mut packages, path, names);
        }
        Self {
            store,
            definitions,
            packages: Some(packages),
        }
    }

    /// Enters `file` as one unit and runs `check` on its unpickler; the
    /// package registry is kept for the next unit.
    fn unit<R>(
        &mut self,
        bytes: &[u8],
        check: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>, &TastyFile<'_>) -> R,
    ) -> R {
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let packages = self.packages.take().expect("packages");
        let mut unpickler =
            TastyUnpickler::with_packages(&file, &mut self.store, self.definitions, packages);
        unpickler.enter_symbols().expect("entering succeeds");
        let result = check(&mut unpickler, &file);
        let (_, packages) = unpickler.into_parts();
        self.packages = Some(packages);
        result
    }
}

/// The addresses of the type definitions named `text`, in address order.
fn typedefs_named(file: &TastyFile<'_>, text: &str) -> Vec<u32> {
    let index = file.ast_address_index().unwrap();
    let mut found = Vec::new();
    for node in index.iter_nodes() {
        let at = u32::try_from(node.offset).unwrap();
        let Some(raw) = index.get(at) else { continue };
        if let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) =
            raw.decode_structured()
            && file.names().get_utf8(name) == Some(text)
        {
            found.push(at);
        }
    }
    found.sort_unstable();
    found
}

/// The class-like definition named `text`: the one whose entered kind is
/// `kind`.
fn class_named(
    unpickler: &TastyUnpickler<'_, '_, '_>,
    file: &TastyFile<'_>,
    text: &str,
    kind: SymbolKind,
) -> u32 {
    typedefs_named(file, text)
        .into_iter()
        .find(|at| unpickler.symbol_state_at(*at).map(|state| state.0) == Some(kind))
        .unwrap_or_else(|| panic!("no {kind:?} named {text}"))
}

fn class_info_of(store: &SemanticStore, ty: TypeId) -> ClassInfo {
    match store.types.get(ty) {
        Type::ClassInfo(info) => info.clone(),
        other => panic!("not a ClassInfo: {other:?}"),
    }
}

fn stub_classes(store: &mut SemanticStore, packages: &mut Packages, path: &[&str], names: &[&str]) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::{Symbol, SymbolFlags, SymbolLinks, Visibility};
    let package = packages
        .enter(store, SymbolOrigin::Synthetic, path)
        .pop()
        .unwrap();
    for class in names {
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
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
        store.scopes.get_mut(package.scope).enter(name, symbol);
    }
}

// ---- class completion skeleton -------------------------------------------

#[test]
fn a_trait_completes_to_a_class_info_with_the_pass_one_scope() {
    let mut session = Session::new();
    let no_prefix = session.definitions.no_prefix;
    let (class, ty, scope, info) = session.unit(BASE, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoBase", SymbolKind::Trait);
        let class = unpickler.index().symbol_at(at).unwrap();
        let scope = unpickler.index().scope_of(class);
        let ty = unpickler.complete_symbol(at).unwrap();
        (class, ty, scope, unpickler.symbol_state_at(at).unwrap().1)
    });
    let info_type = class_info_of(&session.store, ty);
    assert_eq!(info, SymbolInfo::Complete(ty));
    assert_eq!(info_type.class, class);
    assert_eq!(info_type.prefix, no_prefix);
    assert_eq!(Some(info_type.declarations), scope);
    assert_eq!(info_type.self_type, None);
    assert_eq!(
        session.store.scopes.get(info_type.declarations).owner,
        Some(class)
    );
}

#[test]
fn a_class_completes_its_header_type_parameter_before_publishing() {
    let mut session = Session::new();
    let parameter = session.unit(BASE, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoBase", SymbolKind::Trait);
        unpickler.complete_symbol(at).unwrap();
        let class = unpickler.index().symbol_at(at).unwrap();
        (0..64)
            .filter_map(|address| unpickler.symbol_state_at(address))
            .find(|state| state.0 == SymbolKind::TypeParameter)
            .map(|state| (class, state.1))
    });
    let (_, info) = parameter.expect("a type parameter entered");
    assert!(matches!(info, SymbolInfo::Complete(_)));
}

#[test]
fn completing_a_class_twice_returns_the_same_class_info() {
    let mut session = Session::new();
    let (first, second, allocated) = session.unit(BASE, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoBase", SymbolKind::Trait);
        let first = unpickler.complete_symbol(at).unwrap();
        let before = unpickler.index().type_count();
        let second = unpickler.complete_symbol(at).unwrap();
        (first, second, unpickler.index().type_count() - before)
    });
    assert_eq!(first, second);
    assert_eq!(allocated, 0);
}

#[test]
fn a_failed_class_completion_leaves_the_class_missing_and_its_scope_intact() {
    // InfoNeeds's self type names InfoDep, which this session never entered.
    let mut session = Session::new();
    let (error, state, scope) = session.unit(NEEDS, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoNeeds", SymbolKind::Trait);
        let class = unpickler.index().symbol_at(at).unwrap();
        let error = unpickler.complete_symbol(at).unwrap_err();
        (
            error,
            unpickler.symbol_state_at(at).unwrap().1,
            unpickler.index().scope_of(class),
        )
    });
    assert!(
        matches!(&error, UnpickleError::UnresolvedMember { name, .. } if name == "InfoDep"),
        "{error:?}"
    );
    assert_eq!(state, SymbolInfo::Missing);
    assert!(scope.is_some());
}

// ---- parent projection ---------------------------------------------------

/// The name of the symbol a `TypeRef` type designates.
fn referenced_name(store: &SemanticStore, ty: TypeId) -> String {
    let symbol = store
        .types
        .get(ty)
        .reference_symbol()
        .unwrap_or_else(|| panic!("not a symbol reference: {:?}", store.types.get(ty)));
    store
        .names
        .resolve(store.symbols.get(symbol).name.text())
        .to_owned()
}

/// The applied type's constructor name and argument types.
fn applied(store: &SemanticStore, ty: TypeId) -> (String, Vec<TypeId>) {
    match store.types.get(ty) {
        Type::Applied { tycon, args } => (referenced_name(store, *tycon), args.clone()),
        other => panic!("not an Applied: {other:?}"),
    }
}

fn session_with_parents() -> Session {
    let mut session = Session::new();
    for unit in [BASE, DEP, PARENT] {
        session.unit(unit, |_, _| ());
    }
    session
}

#[test]
fn a_generic_class_keeps_its_ordered_generic_parents() {
    let mut session = session_with_parents();
    let (ty, parameter) = session.unit(CHILD, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoChild", SymbolKind::Class);
        let ty = unpickler.complete_symbol(at).unwrap();
        let parameter = (0..64)
            .find_map(|address| {
                let symbol = unpickler.index().symbol_at(address)?;
                (unpickler.symbol_state_at(address)?.0 == SymbolKind::TypeParameter)
                    .then_some(symbol)
            })
            .expect("the class type parameter");
        (ty, parameter)
    });
    let info = class_info_of(&session.store, ty);
    let [parent, base] = info.parents[..] else {
        panic!("two parents: {:?}", info.parents);
    };
    let (parent_name, parent_args) = applied(&session.store, parent);
    let (base_name, base_args) = applied(&session.store, base);
    assert_eq!(parent_name, "InfoParent");
    assert_eq!(base_name, "InfoBase");
    // The argument is the class's own type parameter symbol, not a ParamRef.
    for args in [parent_args, base_args] {
        let [arg] = args[..] else { panic!("one arg") };
        assert_eq!(
            session.store.types.get(arg).reference_symbol(),
            Some(parameter)
        );
    }
}

#[test]
fn a_type_constructor_parameter_applied_in_a_parent_stays_applied() {
    let mut session = session_with_parents();
    let ty = session.unit(WRAPPED, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoWrapped", SymbolKind::Class);
        unpickler.complete_symbol(at).unwrap()
    });
    let info = class_info_of(&session.store, ty);
    // `Object` (the superclass the compiler adds) precedes the trait.
    let [object, base] = info.parents[..] else {
        panic!("two parents");
    };
    assert_eq!(referenced_name(&session.store, object), "Object");
    let (name, args) = applied(&session.store, base);
    assert_eq!(name, "InfoBase");
    let [arg] = args[..] else { panic!("one arg") };
    let (inner, _) = applied(&session.store, arg);
    assert_eq!(inner, "F");
}

#[test]
fn a_class_without_an_extends_clause_has_the_object_parent() {
    let mut session = session_with_parents();
    let ty = session.unit(HOLDER, |unpickler, file| {
        let at = class_named(unpickler, file, "Nested", SymbolKind::Class);
        unpickler.complete_symbol(at).unwrap()
    });
    let info = class_info_of(&session.store, ty);
    let [parent] = info.parents[..] else {
        panic!("one parent");
    };
    let (name, args) = applied(&session.store, parent);
    assert_eq!(name, "InfoParent");
    assert_eq!(args.len(), 1);
}

#[test]
fn class_completion_does_not_force_its_members() {
    let mut session = session_with_parents();
    let states = session.unit(CHILD, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoChild", SymbolKind::Class);
        unpickler.complete_symbol(at).unwrap();
        ["method", "Member"].map(|name| {
            let member = typedefs_or_defs_named(file, name);
            unpickler.symbol_state_at(member).unwrap().1
        })
    });
    assert_eq!(states, [SymbolInfo::Missing, SymbolInfo::Missing]);
}

/// The definition (method or type) named `text`.
fn typedefs_or_defs_named(file: &TastyFile<'_>, text: &str) -> u32 {
    let index = file.ast_address_index().unwrap();
    for node in index.iter_nodes() {
        let at = u32::try_from(node.offset).unwrap();
        let Some(raw) = index.get(at) else { continue };
        let name = match raw.decode_structured() {
            Ok(StructuredNode::DefDef(body)) => body.name,
            Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) => name,
            _ => continue,
        };
        if file.names().get_utf8(name) == Some(text) {
            return at;
        }
    }
    panic!("no definition named {text}")
}

// ---- self types and lambdas in parent / self positions -------------------

fn lambda_of(store: &SemanticStore, ty: TypeId) -> dotty_core::types::TypeLambda {
    match store.types.get(ty) {
        Type::TypeLambda(lambda) => lambda.clone(),
        other => panic!("not a TypeLambda: {other:?}"),
    }
}

/// Whether the scope declares a type named `text`.
fn declares_type(store: &SemanticStore, scope: dotty_core::ids::ScopeId, text: &str) -> bool {
    use dotty_core::names::{Name, Namespace};
    store.names.get(text).is_some_and(|text| {
        !store
            .scopes
            .get(scope)
            .lookup_all(&Name::new(text, Namespace::Type))
            .is_empty()
    })
}

#[test]
fn an_explicit_self_type_is_the_class_info_self_type() {
    let mut session = session_with_parents();
    let ty = session.unit(CHILD, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoChild", SymbolKind::Class);
        unpickler.complete_symbol(at).unwrap()
    });
    let info = class_info_of(&session.store, ty);
    let self_type = info.self_type.expect("an explicit self type");
    assert_eq!(referenced_name(&session.store, self_type), "InfoDep");
}

#[test]
fn a_dependent_self_type_keeps_the_class_type_parameter_symbol() {
    let mut session = session_with_parents();
    let (ty, parameter) = session.unit(GEN_SELF, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoGenSelf", SymbolKind::Class);
        let ty = unpickler.complete_symbol(at).unwrap();
        let parameter = (0..64)
            .find_map(|address| {
                let symbol = unpickler.index().symbol_at(address)?;
                (unpickler.symbol_state_at(address)?.0 == SymbolKind::TypeParameter)
                    .then_some(symbol)
            })
            .expect("the class type parameter");
        (ty, parameter)
    });
    let self_type = class_info_of(&session.store, ty)
        .self_type
        .expect("an explicit self type");
    let (name, args) = applied(&session.store, self_type);
    assert_eq!(name, "InfoBase");
    let [arg] = args[..] else { panic!("one arg") };
    assert_eq!(
        session.store.types.get(arg).reference_symbol(),
        Some(parameter)
    );
}

#[test]
fn a_class_without_a_self_definition_has_no_self_type() {
    let mut session = session_with_parents();
    let ty = session.unit(WRAPPED, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoWrapped", SymbolKind::Class);
        unpickler.complete_symbol(at).unwrap()
    });
    assert_eq!(class_info_of(&session.store, ty).self_type, None);
}

fn session_with_lambda_units() -> Session {
    let mut session = session_with_parents();
    session.unit(LAM, |_, _| ());
    session
}

#[test]
fn a_lambda_in_a_parent_is_a_type_lambda_owned_by_the_class_and_not_a_member() {
    let mut session = session_with_lambda_units();
    let (ty, class, entered) = session.unit(LAMBDA_PARENT, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoLambdaParent", SymbolKind::Class);
        let class = unpickler.index().symbol_at(at).unwrap();
        let entered: Vec<_> = (0..256)
            .filter_map(|address| unpickler.index().symbol_at(address))
            .collect();
        (unpickler.complete_symbol(at).unwrap(), class, entered)
    });
    let info = class_info_of(&session.store, ty);
    let (name, args) = applied(&session.store, *info.parents.last().unwrap());
    assert_eq!(name, "InfoLam");
    let lambda = lambda_of(&session.store, args[0]);
    assert_eq!(lambda.params.len(), 1);
    // The lambda's parameter symbol is owned by the class but is not one of
    // its members.
    assert!(!declares_type(&session.store, info.declarations, "X"));
    let owned: Vec<_> = entered
        .iter()
        .filter(|symbol| {
            let symbol = session.store.symbols.get(**symbol);
            symbol.owner == Some(class)
                && symbol.kind == SymbolKind::TypeParameter
                && session.store.names.resolve(symbol.name.text()) == "X"
        })
        .collect();
    assert_eq!(owned.len(), 1);
}

#[test]
fn a_lambda_in_a_self_type_is_a_type_lambda() {
    let mut session = session_with_lambda_units();
    let ty = session.unit(LAMBDA_SELF, |unpickler, file| {
        let at = class_named(unpickler, file, "InfoLambdaSelf", SymbolKind::Trait);
        unpickler.complete_symbol(at).unwrap()
    });
    let self_type = class_info_of(&session.store, ty)
        .self_type
        .expect("a self type");
    let (name, args) = applied(&session.store, self_type);
    assert_eq!(name, "InfoLam");
    assert_eq!(lambda_of(&session.store, args[0]).params.len(), 1);
}

#[test]
fn a_failure_after_header_work_restores_everything_completion_did() {
    // InfoDep (the self type) is not entered, so completion fails after the
    // header type parameter completed.
    let control = {
        let mut session = Session::new();
        session.unit(NEEDS_GENERIC, |_, _| ());
        session.store.types.alloc(Type::NoType)
    };
    let mut session = Session::new();
    let (error, class_state, parameter_state, scope, in_scope) =
        session.unit(NEEDS_GENERIC, |unpickler, file| {
            let at = class_named(unpickler, file, "InfoNeedsGeneric", SymbolKind::Trait);
            let class = unpickler.index().symbol_at(at).unwrap();
            let error = unpickler.complete_symbol(at).unwrap_err();
            let parameter = (0..64)
                .find(|address| {
                    unpickler
                        .symbol_state_at(*address)
                        .is_some_and(|state| state.0 == SymbolKind::TypeParameter)
                })
                .expect("the header type parameter");
            (
                error,
                unpickler.symbol_state_at(at).unwrap().1,
                unpickler.symbol_state_at(parameter).unwrap().1,
                unpickler.index().scope_of(class),
                unpickler.index().type_count(),
            )
        });
    assert!(matches!(&error, UnpickleError::UnresolvedMember { name, .. } if name == "InfoDep"));
    assert_eq!(class_state, SymbolInfo::Missing);
    // The header parameter was completed by this call, then restored.
    assert_eq!(parameter_state, SymbolInfo::Missing);
    // The pass-1 scope is still there, with the parameter declared in it.
    let scope = scope.expect("the pass-1 scope survives");
    assert!(declares_type(&session.store, scope, "A"));
    // The arena and the unit's caches are as before the call.
    assert_eq!(in_scope, 0);
    assert_eq!(session.store.types.alloc(Type::NoType), control);
}
