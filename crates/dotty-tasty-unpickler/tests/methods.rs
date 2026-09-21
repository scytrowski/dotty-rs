//! Milestone 5c: completing ordinary `DEFDEF` methods, on the real Scala 3.9.0
//! `Methods.tasty` and `ErasedParams.tasty` fixtures.
use std::collections::HashMap;

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind, SymbolOrigin};
use dotty_core::types::{MethodKind, MethodType, PolyType, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{DefinitionBody, StructuredNode, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const METHODS: &[u8] = include_bytes!("fixtures/semantic/Methods.tasty");
const ERASED: &[u8] = include_bytes!("fixtures/semantic/ErasedParams.tasty");

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
}

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

/// A session where the library classes the fixtures use are stubs.
fn entered<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
    with_scala: bool,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    if with_scala {
        stub_classes(
            session,
            &mut packages,
            &["scala"],
            &["Int", "Any", "Boolean", "Nothing", "<repeated>"],
        );
        stub_classes(session, &mut packages, &["scala", "math"], &["Ordering"]);
        stub_classes(session, &mut packages, &["java", "lang"], &["Comparable"]);
    }
    let mut unpickler =
        TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    unpickler
}

/// Every method definition by name, in address order (overloads are two).
fn methods_by_name(file: &TastyFile<'_>) -> HashMap<String, Vec<u32>> {
    let index = file.ast_address_index().unwrap();
    let mut found: HashMap<String, Vec<u32>> = HashMap::new();
    for node in index.iter_nodes() {
        let at = u32::try_from(node.offset).unwrap();
        let Some(raw) = index.get(at) else { continue };
        let name = match raw.decode_structured() {
            Ok(StructuredNode::DefDef(body)) => body.name,
            Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) => name,
            _ => continue,
        };
        if let Some(text) = file.names().get_utf8(name) {
            found.entry(text.to_owned()).or_default().push(at);
        }
    }
    for addresses in found.values_mut() {
        addresses.sort_unstable();
    }
    found
}

/// The parameter node addresses of the `DEFDEF` at `at`, in wire order.
fn parameter_nodes(file: &TastyFile<'_>, at: u32) -> Vec<u32> {
    let index = file.ast_address_index().unwrap();
    index
        .iter_tree_edges()
        .filter(|edge| edge.parent.offset == at as usize && matches!(edge.child.tag, 133 | 134))
        .map(|edge| u32::try_from(edge.child.offset).unwrap())
        .collect()
}

fn method_of(session: &Session, ty: TypeId) -> MethodType {
    match session.store.types.get(ty) {
        Type::Method(method) => method.clone(),
        other => panic!("not a method type: {other:?}"),
    }
}

fn poly_of(session: &Session, ty: TypeId) -> PolyType {
    match session.store.types.get(ty) {
        Type::Poly(poly) => poly.clone(),
        other => panic!("not a poly type: {other:?}"),
    }
}

fn param_ref(session: &Session, ty: TypeId) -> (TypeId, u32) {
    match session.store.types.get(ty) {
        Type::ParamRef { binder, index } => (*binder, *index),
        other => panic!("not a parameter reference: {other:?}"),
    }
}

fn text(session: &Session, name: dotty_core::ids::NameId) -> String {
    session.store.names.resolve(name).to_owned()
}

/// Whether `ty` reaches a reference to one of `symbols`, through the parts a
/// signature is made of.
fn mentions(session: &Session, ty: TypeId, symbols: &[SymbolId]) -> bool {
    let mut stack = vec![ty];
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        match session.store.types.get(current) {
            Type::TypeRef { prefix, target } => {
                if matches!(target, TypeRefTarget::Symbol(s) if symbols.contains(s)) {
                    return true;
                }
                stack.push(*prefix);
            }
            Type::TermRef { prefix, target } => {
                if matches!(target, TermRefTarget::Symbol(s) if symbols.contains(s)) {
                    return true;
                }
                stack.push(*prefix);
            }
            Type::Applied { tycon, args } => {
                stack.push(*tycon);
                stack.extend(args);
            }
            Type::Method(method) => {
                stack.extend(method.params.iter().map(|param| param.ty));
                stack.push(method.result);
            }
            Type::Poly(poly) => {
                stack.extend(poly.params.iter().map(|param| param.bounds));
                stack.push(poly.result);
            }
            Type::Bounds { low, high } => stack.extend([*low, *high]),
            Type::AliasingBounds { alias } => stack.push(*alias),
            Type::ByName { result } => stack.push(*result),
            _ => {}
        }
    }
    false
}

fn complete(
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    methods: &HashMap<String, Vec<u32>>,
    name: &str,
) -> TypeId {
    unpickler
        .complete_symbol(methods[name][0])
        .unwrap_or_else(|error| panic!("{name}: {error:?}"))
}

#[test]
fn no_clause_is_by_name_and_an_empty_clause_is_an_empty_method() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let nullary = complete(&mut unpickler, &methods, "nullary");
    let empty = complete(&mut unpickler, &methods, "empty");
    drop(unpickler);

    // `def nullary: Int` is `ExprType(Int)`, `def empty(): Int` a method.
    assert!(matches!(
        session.store.types.get(nullary),
        Type::ByName { .. }
    ));
    let empty = method_of(&session, empty);
    assert!(empty.params.is_empty());
    assert_eq!(empty.kind, MethodKind::Plain);
    assert!(!matches!(
        session.store.types.get(empty.result),
        Type::ByName { .. }
    ));
}

#[test]
fn one_term_clause_is_a_plain_method_with_erased_and_varargs_false() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let one = complete(&mut unpickler, &methods, "one");
    drop(unpickler);
    let method = method_of(&session, one);
    assert_eq!(method.kind, MethodKind::Plain);
    assert_eq!(method.params.len(), 1);
    assert_eq!(text(&session, method.params[0].name.as_name().text()), "x");
    assert!(!method.params[0].erased);
    assert!(!method.params[0].varargs);
}

#[test]
fn curried_clauses_nest_in_source_order_with_a_binder_each() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let curried = complete(&mut unpickler, &methods, "curried");
    drop(unpickler);
    let outer = method_of(&session, curried);
    assert_eq!(text(&session, outer.params[0].name.as_name().text()), "x");
    let inner = method_of(&session, outer.result);
    assert_eq!(text(&session, inner.params[0].name.as_name().text()), "y");
    assert_ne!(outer.result, curried);
}

#[test]
fn a_type_clause_is_a_poly_and_its_references_are_param_refs_of_that_poly() {
    // `def contextual[A](x: A)(using ord: Ordering[A]): A`
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);
    let params = parameter_nodes(&file, methods["contextual"][0]);
    let symbols: Vec<SymbolId> = params
        .iter()
        .map(|at| unpickler.index().symbol_at(*at).unwrap())
        .collect();

    let contextual = complete(&mut unpickler, &methods, "contextual");
    drop(unpickler);

    let poly = poly_of(&session, contextual);
    assert_eq!(poly.params.len(), 1);
    assert_eq!(text(&session, poly.params[0].name.as_name().text()), "A");
    assert_eq!(poly.params[0].declared_variance, None);
    let first = method_of(&session, poly.result);
    assert_eq!(first.kind, MethodKind::Plain);
    assert_eq!(param_ref(&session, first.params[0].ty), (contextual, 0));
    let second = method_of(&session, first.result);
    assert_eq!(second.kind, MethodKind::Contextual);
    let Type::Applied { args, .. } = session.store.types.get(second.params[0].ty).clone() else {
        panic!("not applied");
    };
    assert_eq!(param_ref(&session, args[0]), (contextual, 0));
    assert_eq!(param_ref(&session, second.result), (contextual, 0));
    // No parameter symbol is reachable from the signature any more.
    assert!(!mentions(&session, contextual, &symbols));
}

#[test]
fn an_f_bound_names_the_poly_of_the_method() {
    // `def fbounded[A <: Comparable[A]](x: A): A`
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);
    let a = unpickler
        .index()
        .symbol_at(parameter_nodes(&file, methods["fbounded"][0])[0])
        .unwrap();

    let fbounded = complete(&mut unpickler, &methods, "fbounded");
    drop(unpickler);

    let poly = poly_of(&session, fbounded);
    let Type::Bounds { high, .. } = session.store.types.get(poly.params[0].bounds).clone() else {
        panic!("not bounds");
    };
    let Type::Applied { args, .. } = session.store.types.get(high).clone() else {
        panic!("not applied");
    };
    assert_eq!(param_ref(&session, args[0]), (fbounded, 0));
    assert!(!mentions(&session, fbounded, &[a]));
    let method = method_of(&session, poly.result);
    assert_eq!(param_ref(&session, method.params[0].ty), (fbounded, 0));
    assert_eq!(param_ref(&session, method.result), (fbounded, 0));
}

#[test]
fn a_dependent_result_names_the_parameter_by_param_ref() {
    // `def dependent(x: Box): x.Out`
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);
    let x = unpickler
        .index()
        .symbol_at(parameter_nodes(&file, methods["dependent"][0])[0])
        .unwrap();
    let out = unpickler.index().symbol_at(methods["Out"][0]).unwrap();

    let dependent = complete(&mut unpickler, &methods, "dependent");
    drop(unpickler);

    let method = method_of(&session, dependent);
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(member),
    } = session.store.types.get(method.result).clone()
    else {
        panic!("not a selection");
    };
    assert_eq!(member, out);
    assert_eq!(param_ref(&session, prefix), (dependent, 0));
    assert!(!mentions(&session, dependent, &[x]));
}

#[test]
fn a_later_clause_refers_to_an_earlier_one_and_its_own_parameters() {
    // `def crossClause(x: Box)(y: x.Out): y.type`
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);
    let params = parameter_nodes(&file, methods["crossClause"][0]);
    let symbols: Vec<SymbolId> = params
        .iter()
        .map(|at| unpickler.index().symbol_at(*at).unwrap())
        .collect();

    let cross = complete(&mut unpickler, &methods, "crossClause");
    drop(unpickler);

    let outer = method_of(&session, cross);
    let inner_id = outer.result;
    let inner = method_of(&session, inner_id);
    // `y: x.Out`: the prefix is the outer binder, not the symbol `x`.
    let Type::TypeRef { prefix, .. } = session.store.types.get(inner.params[0].ty).clone() else {
        panic!("not a selection");
    };
    assert_eq!(param_ref(&session, prefix), (cross, 0));
    // `y.type`: the inner binder.
    assert_eq!(param_ref(&session, inner.result), (inner_id, 0));
    assert!(!mentions(&session, cross, &symbols));
}

#[test]
fn an_implicit_clause_is_implicit_a_using_clause_contextual_and_by_name_is_kept() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let implicit = complete(&mut unpickler, &methods, "implicitClause");
    let by_name = complete(&mut unpickler, &methods, "byNameParam");
    let type_only = complete(&mut unpickler, &methods, "typeOnly");
    drop(unpickler);

    let outer = method_of(&session, implicit);
    assert_eq!(outer.kind, MethodKind::Plain);
    assert_eq!(method_of(&session, outer.result).kind, MethodKind::Implicit);
    let method = method_of(&session, by_name);
    assert!(matches!(
        session.store.types.get(method.params[0].ty),
        Type::ByName { .. }
    ));
    // `def typeOnly[A]: A` has a clause, so it is a poly and not an `ExprType`.
    let poly = poly_of(&session, type_only);
    assert_eq!(param_ref(&session, poly.result), (type_only, 0));
}

#[test]
fn overloads_each_get_their_own_exact_info() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    assert_eq!(methods["overloaded"].len(), 2);
    let first = unpickler.complete_symbol(methods["overloaded"][0]).unwrap();
    let second = unpickler.complete_symbol(methods["overloaded"][1]).unwrap();
    drop(unpickler);
    assert_ne!(first, second);
    let (a, b) = (method_of(&session, first), method_of(&session, second));
    assert_ne!(a.params[0].ty, b.params[0].ty);
}

#[test]
fn completing_a_method_twice_returns_the_same_type_and_allocates_nothing() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let first = complete(&mut unpickler, &methods, "crossClause");
    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    assert_eq!(
        unpickler.complete_symbol(methods["crossClause"][0]),
        Ok(first)
    );
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().type_tree_count(), trees);
    // The parameters completed by the first call stay complete.
    for at in parameter_nodes(&file, methods["crossClause"][0]) {
        assert!(matches!(
            unpickler.symbol_state_at(at).unwrap().1,
            SymbolInfo::Complete(_)
        ));
    }
}

#[test]
fn the_parameters_and_the_projected_result_keep_naming_the_symbols() {
    // Abstraction builds binder types; the graphs the parameters and the
    // result tree were projected to are not touched.
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);
    let params = parameter_nodes(&file, methods["crossClause"][0]);
    let symbols: Vec<SymbolId> = params
        .iter()
        .map(|at| unpickler.index().symbol_at(*at).unwrap())
        .collect();

    complete_method_and_look(&mut unpickler, &methods, "crossClause");
    let y_info = match unpickler.symbol_state_at(params[1]).unwrap().1 {
        SymbolInfo::Complete(ty) => ty,
        other => panic!("{other:?}"),
    };
    drop(unpickler);
    // `y: x.Out` as the parameter's own info still names the symbol `x`.
    assert!(mentions(&session, y_info, &symbols[..1]));
}

fn complete_method_and_look(
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    methods: &HashMap<String, Vec<u32>>,
    name: &str,
) {
    complete(unpickler, methods, name);
}

#[test]
fn a_method_with_an_unresolved_type_stays_missing_and_undoes_its_parameters() {
    // No `scala` package is known: `Int` in the parameter cannot resolve.
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, false);

    let types = unpickler.index().type_count();
    let result = unpickler.complete_symbol(methods["one"][0]);
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. })
        ),
        "{result:?}"
    );
    assert_eq!(
        unpickler.symbol_state_at(methods["one"][0]).unwrap().1,
        SymbolInfo::Missing
    );
    for at in parameter_nodes(&file, methods["one"][0]) {
        assert_eq!(
            unpickler.symbol_state_at(at).unwrap().1,
            SymbolInfo::Missing
        );
    }
    assert_eq!(unpickler.index().type_count(), types);
    // A retry fails the same way, and another method still completes.
    assert_eq!(unpickler.complete_symbol(methods["one"][0]), result);
}

#[test]
fn constructors_stay_missing_with_a_typed_deferral() {
    let file = TastyFile::parse_scala_3_9(METHODS).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let constructor = methods["<init>"][0];
    assert_eq!(
        unpickler.complete_symbol(constructor),
        Err(UnpickleError::ConstructorCompletionDeferred {
            address: constructor
        })
    );
    assert_eq!(
        unpickler.symbol_state_at(constructor),
        Some((SymbolKind::Constructor, SymbolInfo::Missing))
    );
}

#[test]
fn an_erased_parameter_is_erased_by_its_flag_and_only_it() {
    // `def proof(erased x: Int, y: Int): Int`
    let file = TastyFile::parse_scala_3_9(ERASED).unwrap();
    let methods = methods_by_name(&file);
    let mut session = Session::new();
    let mut unpickler = entered(&file, &mut session, true);

    let proof = unpickler.complete_symbol(methods["proof"][0]).unwrap();
    drop(unpickler);
    let method = method_of(&session, proof);
    assert_eq!(
        method.params.iter().map(|p| p.erased).collect::<Vec<_>>(),
        vec![true, false]
    );
    assert!(method.params.iter().all(|p| !p.varargs));
}

// Synthetic wire: clause markers, modifiers, refusals and rollback

mod wire {
    use super::*;
    use dotty_tasty::tasty::{
        Header, NameTable, PACKAGE_TAG, RawName, Section, SectionTable, TEMPLATE_TAG,
        TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TYPEREFPKG_TAG,
    };

    const NAMES: [&str; 23] = [
        "ASTs",
        "p",
        "Holder",
        "split",
        "empties",
        "inline",
        "tracked",
        "into",
        "given",
        "implicit",
        "erased",
        "covariant",
        "other",
        "dep",
        "late",
        "a",
        "b",
        "z",
        "A",
        "T",
        "Out",
        "mixed",
        "c",
    ];

    fn n(text: &str) -> u32 {
        u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
    }

    const DEFDEF: u8 = 130;
    const PARAM: u8 = 134;
    const IDENTTPT: u8 = 111;
    const SELECTTPT: u8 = 113;
    const REFINEDTPT: u8 = 160;
    const EMPTYCLAUSE: u8 = 45;
    const SPLITCLAUSE: u8 = 46;
    const GIVEN: u8 = 37;
    const IMPLICIT: u8 = 13;
    const ERASED_MOD: u8 = 34;
    const INLINE: u8 = 17;
    const TRACKED: u8 = 47;
    const INTO: u8 = 49;
    const COVARIANT: u8 = 28;

    pub const DEFINITIONS: [&str; 34] = [
        "Holder",
        "split",
        "split.a",
        "split.b",
        "empties",
        "empties.a",
        "inline",
        "inline.a",
        "tracked",
        "tracked.a",
        "into",
        "into.a",
        "given",
        "given.a",
        "given.b",
        "implicit",
        "implicit.a",
        "erased",
        "erased.a",
        "erased.b",
        "covariant",
        "covariant.A",
        "other",
        "other.z",
        "dep",
        "dep.a",
        "late",
        "late.T",
        "late.a",
        "late.b",
        "mixed",
        "mixed.a",
        "mixed.b",
        "mixed.c",
    ];

    fn nat(value: u32) -> Vec<u8> {
        let mut groups = vec![u8::try_from(value & 0x7f).unwrap() | 0x80];
        let mut rest = value >> 7;
        while rest > 0 {
            groups.push(u8::try_from(rest & 0x7f).unwrap());
            rest >>= 7;
        }
        groups.reverse();
        groups
    }

    fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend(nat(u32::try_from(payload.len()).unwrap()));
        bytes.extend(payload);
        bytes
    }

    fn leaf(tag: u8, value: u32) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend(nat(value));
        bytes
    }

    fn named(tag: u8, name: u32, child: &[u8]) -> Vec<u8> {
        [&[tag][..], &nat(name), child].concat()
    }

    fn any_type() -> Vec<u8> {
        leaf(TYPEREFPKG_TAG, n("p"))
    }

    fn ident_any() -> Vec<u8> {
        named(IDENTTPT, n("p"), &any_type())
    }

    /// `PARAM name tpt modifiers`.
    fn param(name: &str, tpt: Vec<u8>, modifiers: &[u8]) -> Vec<u8> {
        node(PARAM, &[nat(n(name)), tpt, modifiers.to_vec()].concat())
    }

    fn def(name: &str, header: Vec<Vec<u8>>, result: Vec<u8>) -> Vec<u8> {
        node(DEFDEF, &[nat(n(name)), header.concat(), result].concat())
    }

    fn file_with(ast: &[u8]) -> Vec<u8> {
        let names = NameTable::from_entries(
            NAMES
                .iter()
                .map(|text| RawName::Utf8((*text).to_owned()))
                .collect(),
        )
        .unwrap();
        TastyFile::from_parts(
            Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names,
            SectionTable::from_sections(vec![Section::new(0, ast)]),
        )
        .unwrap()
        .encode()
        .unwrap()
    }

    fn assemble(at: &HashMap<&'static str, u32>) -> Vec<u8> {
        let addr = |label: &str| at.get(label).copied().unwrap_or(0);
        let members = [
            any_type(),
            def(
                "split",
                vec![
                    param("a", ident_any(), &[]),
                    vec![SPLITCLAUSE],
                    param("b", ident_any(), &[]),
                ],
                ident_any(),
            ),
            def(
                "empties",
                vec![
                    vec![EMPTYCLAUSE],
                    param("a", ident_any(), &[]),
                    vec![EMPTYCLAUSE],
                ],
                ident_any(),
            ),
            def(
                "inline",
                vec![param("a", ident_any(), &[INLINE])],
                ident_any(),
            ),
            def(
                "tracked",
                vec![param("a", ident_any(), &[TRACKED])],
                ident_any(),
            ),
            def("into", vec![param("a", ident_any(), &[INTO])], ident_any()),
            def(
                "given",
                vec![
                    param("a", ident_any(), &[GIVEN]),
                    param("b", ident_any(), &[]),
                ],
                ident_any(),
            ),
            def(
                "implicit",
                vec![param("a", ident_any(), &[IMPLICIT])],
                ident_any(),
            ),
            def(
                "erased",
                vec![
                    param("a", ident_any(), &[ERASED_MOD]),
                    param("b", ident_any(), &[]),
                ],
                ident_any(),
            ),
            def(
                "covariant",
                vec![node(
                    TYPEPARAM_TAG,
                    &[
                        nat(n("A")),
                        node(
                            dotty_tasty::tasty::TYPEBOUNDSTPT_TAG,
                            &[any_type(), any_type()].concat(),
                        ),
                        vec![COVARIANT],
                    ]
                    .concat(),
                )],
                ident_any(),
            ),
            def("other", vec![param("z", ident_any(), &[])], ident_any()),
            // `def dep(a: p): other.z.Out`: through a parameter of a method
            // that is not completed.
            def(
                "dep",
                vec![param("a", ident_any(), &[])],
                named(
                    SELECTTPT,
                    n("Out"),
                    &leaf(TERMREFDIRECT_TAG, addr("other.z")),
                ),
            ),
            def(
                "late",
                vec![
                    node(
                        TYPEPARAM_TAG,
                        &[
                            nat(n("T")),
                            node(
                                dotty_tasty::tasty::TYPEBOUNDSTPT_TAG,
                                &[any_type(), any_type()].concat(),
                            ),
                        ]
                        .concat(),
                    ),
                    param("a", ident_any(), &[]),
                    vec![SPLITCLAUSE],
                    param("b", node(REFINEDTPT, &any_type()), &[]),
                ],
                ident_any(),
            ),
            def(
                "mixed",
                vec![
                    param("a", ident_any(), &[GIVEN]),
                    param("b", ident_any(), &[IMPLICIT]),
                    param("c", ident_any(), &[]),
                ],
                ident_any(),
            ),
        ];
        let holder = node(
            TYPEDEF_TAG,
            &[nat(n("Holder")), node(TEMPLATE_TAG, &members.concat())].concat(),
        );
        node(
            PACKAGE_TAG,
            &[leaf(TERMREFPKG_TAG, n("p")), holder].concat(),
        )
    }

    pub struct Unit {
        pub bytes: Vec<u8>,
        pub at: HashMap<&'static str, u32>,
    }

    impl Unit {
        pub fn at(&self, label: &str) -> u32 {
            self.at[label]
        }

        pub fn new() -> Self {
            let mut at: HashMap<&'static str, u32> = HashMap::new();
            for _ in 0..8 {
                let bytes = file_with(&assemble(&at));
                let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
                let mut definitions: Vec<u32> = file
                    .ast_address_index()
                    .unwrap()
                    .iter_nodes()
                    .filter(|node| matches!(node.tag, TYPEDEF_TAG | DEFDEF | PARAM | TYPEPARAM_TAG))
                    .map(|node| u32::try_from(node.offset).unwrap())
                    .collect();
                definitions.sort_unstable();
                assert_eq!(definitions.len(), DEFINITIONS.len());
                let next: HashMap<&'static str, u32> =
                    DEFINITIONS.iter().copied().zip(definitions).collect();
                if next == at {
                    return Self { bytes, at };
                }
                at = next;
            }
            panic!("the layout did not settle");
        }
    }
}

fn wire_session<'a>(
    file: &'a TastyFile<'a>,
    session: &'a mut Session,
) -> TastyUnpickler<'a, 'a, 'a> {
    let mut packages = Packages::new();
    packages.enter(&mut session.store, SymbolOrigin::Synthetic, &["p"]);
    let mut unpickler =
        TastyUnpickler::with_packages(file, &mut session.store, session.definitions, packages);
    unpickler.enter_symbols().unwrap();
    unpickler
}

#[test]
fn a_split_clause_keeps_two_term_clauses_and_empty_clauses_are_kept_in_place() {
    let unit = wire::Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = wire_session(&file, &mut session);

    let split = unpickler.complete_symbol(unit.at("split")).unwrap();
    let empties = unpickler.complete_symbol(unit.at("empties")).unwrap();
    drop(unpickler);

    let outer = method_of(&session, split);
    assert_eq!(outer.params.len(), 1);
    assert_eq!(method_of(&session, outer.result).params.len(), 1);
    // `()(a)()`: empty, one, empty.
    let first = method_of(&session, empties);
    assert!(first.params.is_empty());
    let second = method_of(&session, first.result);
    assert_eq!(second.params.len(), 1);
    assert!(method_of(&session, second.result).params.is_empty());
}

#[test]
fn a_parameter_modifier_with_an_unmodeled_adaptation_defers_the_method() {
    let unit = wire::Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = wire_session(&file, &mut session);

    for (label, tag) in [("inline", 17), ("tracked", 47), ("into", 49)] {
        assert_eq!(
            unpickler.complete_symbol(unit.at(label)),
            Err(UnpickleError::UnsupportedMethodParameterSemantics {
                address: unit.at(&format!("{label}.a")),
                tag
            }),
            "{label}"
        );
        // Nothing was completed, not even the parameter.
        assert_eq!(
            unpickler.symbol_state_at(unit.at(label)).unwrap().1,
            SymbolInfo::Missing
        );
        assert_eq!(
            unpickler
                .symbol_state_at(unit.at(&format!("{label}.a")))
                .unwrap()
                .1,
            SymbolInfo::Missing
        );
    }
}

#[test]
fn the_first_parameter_of_a_clause_decides_its_kind_and_only_erased_is_erased() {
    let unit = wire::Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = wire_session(&file, &mut session);

    let given = unpickler.complete_symbol(unit.at("given")).unwrap();
    let implicit = unpickler.complete_symbol(unit.at("implicit")).unwrap();
    let mixed = unpickler.complete_symbol(unit.at("mixed")).unwrap();
    let erased = unpickler.complete_symbol(unit.at("erased")).unwrap();
    drop(unpickler);

    assert_eq!(method_of(&session, given).kind, MethodKind::Contextual);
    assert_eq!(method_of(&session, implicit).kind, MethodKind::Implicit);
    // Dotty's rule: `a` is `given`, so the whole clause is contextual; the
    // `implicit` on `b` is not a majority vote.
    let mixed = method_of(&session, mixed);
    assert_eq!(mixed.kind, MethodKind::Contextual);
    assert_eq!(mixed.params.len(), 3);
    let erased = method_of(&session, erased);
    assert_eq!(
        erased
            .params
            .iter()
            .map(|param| param.erased)
            .collect::<Vec<_>>(),
        vec![true, false]
    );
}

#[test]
fn a_method_type_parameter_has_no_declared_variance_even_with_a_variance_marker() {
    let unit = wire::Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = wire_session(&file, &mut session);

    let covariant = unpickler.complete_symbol(unit.at("covariant")).unwrap();
    drop(unpickler);
    assert_eq!(
        poly_of(&session, covariant).params[0].declared_variance,
        None
    );
}

#[test]
fn completion_never_forces_another_method_so_a_dependency_is_not_a_cycle() {
    // `dep`'s result selects from a parameter of `other`, which is `Missing`:
    // the reference is not resolved by completing `other`.
    let unit = wire::Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = wire_session(&file, &mut session);

    let result = unpickler.complete_symbol(unit.at("dep"));
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{result:?}"
    );
    for label in ["other", "other.z", "dep", "dep.a"] {
        assert_eq!(
            unpickler.symbol_state_at(unit.at(label)).unwrap().1,
            SymbolInfo::Missing,
            "{label}"
        );
    }
    // After the caller completes `other` explicitly, the dependency resolves
    // to what it can: the selected member is still missing in `p`.
    unpickler.complete_symbol(unit.at("other")).unwrap();
}

#[test]
fn a_failure_after_type_and_term_clauses_completed_undoes_them_all_and_retries_cleanly() {
    // `def late[T](a: p)(b: <refined>): p`: `T` and `a` complete, `b` fails.
    let unit = wire::Unit::new();
    let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
    let mut session = Session::new();
    let mut unpickler = wire_session(&file, &mut session);
    // A method completed before must survive the failed call.
    let split = unpickler.complete_symbol(unit.at("split")).unwrap();

    let types = unpickler.index().type_count();
    let trees = unpickler.index().type_tree_count();
    let result = unpickler.complete_symbol(unit.at("late"));
    assert!(
        matches!(
            result,
            Err(UnpickleError::UnsupportedTypeTree { tag: 160, .. })
        ),
        "{result:?}"
    );
    for label in ["late", "late.T", "late.a", "late.b"] {
        assert_eq!(
            unpickler.symbol_state_at(unit.at(label)).unwrap().1,
            SymbolInfo::Missing,
            "{label}"
        );
    }
    assert_eq!(unpickler.index().type_count(), types);
    assert_eq!(unpickler.index().type_tree_count(), trees);
    assert_eq!(unpickler.complete_symbol(unit.at("late")), result);
    assert_eq!(unpickler.complete_symbol(unit.at("split")), Ok(split));
}
