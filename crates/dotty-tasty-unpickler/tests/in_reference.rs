//! `TYPEREFin` / `TERMREFin` (Milestone 4c1): a reference whose declaration is
//! found in an explicit owner space, not among the members of its prefix.
//!
//! The unit is hand-built wire, because a reference to a private or shadowed
//! symbol of *another* compilation unit is what the compiler writes in this
//! form, and needs two units to produce. The wire shapes are those of
//! Scala 3.9.0's `TreePickler`:
//!
//! ```text
//! TYPEREFin Length NameRef prefix_Type ownerSpace_Type
//! TERMREFin Length NameRef prefix_Type ownerSpace_Type
//! ```
//!
//! Package `p` declares the classes `A { T, x, m }`, `B { T, x }` and
//! `D { x, x, T, T }`. Each class holds a same-named type `T` and term `x`, so
//! only the owner space can tell them apart.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use dotty_core::Definitions;
use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::resolution::{
    MemberRequest, MemberSelector, MemberSpace, ResolutionError, SymbolResolver,
};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    DEFDEF_TAG, FLEXIBLETYPE_TAG, Header, MUTABLE_TAG, NameTable, PACKAGE_TAG, POLYTYPE_TAG,
    RawName, SHAREDTYPE_TAG, Section, SectionTable, TEMPLATE_TAG, TERMREFDIRECT_TAG, TERMREFIN_TAG,
    TERMREFPKG_TAG, TYPEBOUNDS_TAG, TYPEDEF_TAG, TYPEREFDIRECT_TAG, TYPEREFIN_TAG, TYPEREFPKG_TAG,
    TastyFile, VALDEF_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

// Name table indices.
const N_P: u32 = 1;
const N_A: u32 = 2;
const N_B: u32 = 3;
const N_T: u32 = 4;
const N_X: u32 = 5;
const N_D: u32 = 6;
const N_M: u32 = 7;
const N_SIGNED_X: u32 = 8;
const N_MISSING: u32 = 9;
const N_LAMBDA: u32 = 10;
const N_V: u32 = 11;

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

fn any_type() -> Vec<u8> {
    leaf(TYPEREFPKG_TAG, N_P)
}

fn member(tag: u8, name: u32) -> Vec<u8> {
    node(tag, &[nat(name), any_type()].concat())
}

fn class(name: u32, members: &[Vec<u8>]) -> Vec<u8> {
    let template = node(
        TEMPLATE_TAG,
        &[vec![any_type()], members.to_vec()].concat().concat(),
    );
    node(TYPEDEF_TAG, &[nat(name), template].concat())
}

fn file_with(ast: &[u8]) -> Vec<u8> {
    let names = NameTable::from_entries(vec![
        RawName::Utf8("ASTs".to_owned()),
        RawName::Utf8("p".to_owned()),
        RawName::Utf8("A".to_owned()),
        RawName::Utf8("B".to_owned()),
        RawName::Utf8("T".to_owned()),
        RawName::Utf8("x".to_owned()),
        RawName::Utf8("D".to_owned()),
        RawName::Utf8("m".to_owned()),
        RawName::Signed {
            original: N_X,
            result_signature: N_T,
            parameter_signatures: vec![],
        },
        RawName::Utf8("Missing".to_owned()),
        RawName::Utf8("L".to_owned()),
        RawName::Utf8("v".to_owned()),
    ])
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

/// A `TYPEREFin` / `TERMREFin` node.
fn reference(tag: u8, name: u32, prefix: &[u8], space: &[u8]) -> Vec<u8> {
    node(tag, &[nat(name), prefix.to_vec(), space.to_vec()].concat())
}

fn shared(address: u32) -> Vec<u8> {
    leaf(SHAREDTYPE_TAG, address)
}

/// The definitions, in document order.
const DEFINITIONS: [&str; 13] = [
    "A", "A.T", "A.x", "A.m", "A.v", "B", "B.T", "B.x", "D", "D.x1", "D.x2", "D.T1", "D.T2",
];

/// The unit's wire and the addresses its tests refer to.
struct Unit {
    bytes: Vec<u8>,
    /// Definition addresses by label.
    defs: HashMap<&'static str, u32>,
    /// Root type addresses by label.
    roots: HashMap<&'static str, u32>,
}

fn package() -> Vec<u8> {
    let members = |names: &[(u32, u8)]| -> Vec<Vec<u8>> {
        names
            .iter()
            .map(|(name, tag)| member(*tag, *name))
            .collect()
    };
    let classes = [
        class(
            N_A,
            &[
                members(&[(N_T, TYPEDEF_TAG), (N_X, VALDEF_TAG), (N_M, DEFDEF_TAG)]),
                // A `var`: a `VALDEF` with the `MUTABLE` modifier.
                vec![node(
                    VALDEF_TAG,
                    &[nat(N_V), any_type(), vec![MUTABLE_TAG]].concat(),
                )],
            ]
            .concat(),
        ),
        class(N_B, &members(&[(N_T, TYPEDEF_TAG), (N_X, VALDEF_TAG)])),
        class(
            N_D,
            &members(&[
                (N_X, VALDEF_TAG),
                (N_X, VALDEF_TAG),
                (N_T, TYPEDEF_TAG),
                (N_T, TYPEDEF_TAG),
            ]),
        ),
    ];
    let payload = [leaf(TERMREFPKG_TAG, N_P), classes.concat()].concat();
    node(PACKAGE_TAG, &payload)
}

impl Unit {
    fn new() -> Self {
        let package = package();
        // The definition addresses come from the unit's own index.
        let first_bytes = file_with(&package);
        let first = TastyFile::parse_scala_3_9(&first_bytes).unwrap();
        let mut addresses: Vec<u32> = first
            .ast_address_index()
            .unwrap()
            .iter_nodes()
            .filter(|node| matches!(node.tag, TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG))
            .map(|node| u32::try_from(node.offset).unwrap())
            .collect();
        addresses.sort_unstable();
        assert_eq!(addresses.len(), DEFINITIONS.len());
        let defs: HashMap<&'static str, u32> = DEFINITIONS.iter().copied().zip(addresses).collect();

        let direct = |tag: u8, label: &str| leaf(tag, defs[label]);
        let (a, b, d) = (
            direct(TYPEREFDIRECT_TAG, "A"),
            direct(TYPEREFDIRECT_TAG, "B"),
            direct(TYPEREFDIRECT_TAG, "D"),
        );
        let mut ast = package;
        let mut roots = HashMap::new();
        fn add(
            ast: &mut Vec<u8>,
            roots: &mut HashMap<&'static str, u32>,
            label: &'static str,
            root: Vec<u8>,
        ) {
            roots.insert(label, u32::try_from(ast.len()).unwrap());
            ast.extend(root);
        }
        let type_in =
            |name, prefix: &[u8], space: &[u8]| reference(TYPEREFIN_TAG, name, prefix, space);
        let term_in =
            |name, prefix: &[u8], space: &[u8]| reference(TERMREFIN_TAG, name, prefix, space);
        add(&mut ast, &mut roots, "type", type_in(N_T, &a, &b));
        add(&mut ast, &mut roots, "term", term_in(N_X, &a, &b));
        let type_at = roots["type"];
        add(&mut ast, &mut roots, "type again", type_in(N_T, &a, &b));
        // A top-level link is not a tree of its own: a wrapper holds it.
        add(
            &mut ast,
            &mut roots,
            "shared type",
            node(FLEXIBLETYPE_TAG, &shared(type_at)),
        );
        add(&mut ast, &mut roots, "signed", term_in(N_SIGNED_X, &a, &b));
        add(&mut ast, &mut roots, "duplicate term", term_in(N_X, &a, &d));
        add(&mut ast, &mut roots, "duplicate type", type_in(N_T, &a, &d));
        add(
            &mut ast,
            &mut roots,
            "term named as type",
            type_in(N_X, &a, &b),
        );
        add(
            &mut ast,
            &mut roots,
            "type named as term",
            term_in(N_T, &a, &b),
        );
        add(&mut ast, &mut roots, "missing", type_in(N_MISSING, &a, &b));
        add(
            &mut ast,
            &mut roots,
            "missing term",
            term_in(N_MISSING, &a, &b),
        );
        // A space that is a type alias has no declarations of its own.
        add(
            &mut ast,
            &mut roots,
            "alias space",
            type_in(N_T, &a, &direct(TYPEREFDIRECT_TAG, "B.T")),
        );
        // An unstable singleton (a method) is not a legal prefix.
        add(
            &mut ast,
            &mut roots,
            "illegal prefix",
            type_in(N_T, &direct(TERMREFDIRECT_TAG, "A.m"), &b),
        );
        // So is a mutable member, which pass 1 enters as a `Field`.
        add(
            &mut ast,
            &mut roots,
            "mutable prefix",
            type_in(N_T, &direct(TERMREFDIRECT_TAG, "A.v"), &b),
        );
        // A binder still being decoded, as the prefix and as the space.
        let mut lambda = |ast: &mut Vec<u8>, label: &'static str, prefix: bool| {
            let at = u32::try_from(ast.len()).unwrap();
            let binder = shared(at);
            let inner = if prefix {
                type_in(N_T, &binder, &b)
            } else {
                type_in(N_T, &a, &binder)
            };
            let mut param = node(TYPEBOUNDS_TAG, &inner);
            param.extend(nat(N_LAMBDA));
            roots.insert(label, at);
            ast.extend(node(POLYTYPE_TAG, &[any_type(), param].concat()));
        };
        lambda(&mut ast, "pending prefix", true);
        lambda(&mut ast, "pending space", false);

        Self {
            bytes: file_with(&ast),
            defs,
            roots,
        }
    }
}

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

/// What a scripted resolver was asked and what it answers.
#[derive(Default)]
struct Script {
    asked: Vec<MemberRequest>,
    answer: Option<SymbolId>,
}

struct Scripted(Rc<RefCell<Script>>);

impl SymbolResolver for Scripted {
    fn resolve_member(
        &mut self,
        _store: &SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let mut script = self.0.borrow_mut();
        script.asked.push(request.clone());
        Ok(script.answer)
    }

    fn resolve_package(
        &mut self,
        _store: &SemanticStore,
        _path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }
}

/// The unit's symbols, by label.
struct Labels(HashMap<&'static str, SymbolId>);

impl Labels {
    fn get(&self, label: &str) -> SymbolId {
        self.0[label]
    }
}

/// A decoding session over the unit. The unit's symbols are entered once, by
/// [`run`](Self::run), so the ids the body sees are the ones the store keeps.
struct Decoder<'u> {
    unit: &'u Unit,
    session: Session,
    script: Rc<RefCell<Script>>,
}

impl<'u> Decoder<'u> {
    fn new(unit: &'u Unit) -> Self {
        Self::with_foreign(unit, &[]).0
    }

    /// A decoder in which `foreign` (name, namespace, kind, owner label)
    /// symbols exist before the unit is entered; their ids are returned. An
    /// owner is one of the unit's symbols, whose id is that of an identical
    /// dry run, since symbols are allocated in a fixed order.
    fn with_foreign(
        unit: &'u Unit,
        foreign: &[(&str, Namespace, SymbolKind, Option<&str>)],
    ) -> (Self, Vec<SymbolId>) {
        // A dry run finds the ids the unit's symbols will get.
        let mut dry = Session::new();
        for (text, namespace, kind, _) in foreign {
            add_symbol(&mut dry.store, text, *namespace, *kind, None);
        }
        let entered = {
            let file = TastyFile::parse_scala_3_9(&unit.bytes).unwrap();
            let mut unpickler = TastyUnpickler::new(&file, &mut dry.store, dry.definitions);
            unpickler.enter_symbols().unwrap();
            let index = unpickler.into_index();
            let labels: HashMap<&'static str, SymbolId> = DEFINITIONS
                .iter()
                .map(|label| (*label, index.symbol_at(unit.defs[label]).unwrap()))
                .collect();
            labels
        };

        let mut session = Session::new();
        let ids: Vec<SymbolId> = foreign
            .iter()
            .map(|(text, namespace, kind, owner)| {
                let owner = owner.map(|label| entered[label]);
                add_symbol(&mut session.store, text, *namespace, *kind, owner)
            })
            .collect();
        let decoder = Self {
            unit,
            session,
            script: Rc::default(),
        };
        (decoder, ids)
    }

    /// Enters the unit and runs `body` with its unpickler and symbols.
    fn run<R>(
        &mut self,
        body: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>, &Labels) -> R,
    ) -> (R, Labels) {
        let file = TastyFile::parse_scala_3_9(&self.unit.bytes).unwrap();
        let mut unpickler =
            TastyUnpickler::new(&file, &mut self.session.store, self.session.definitions)
                .with_resolver(Box::new(Scripted(Rc::clone(&self.script))));
        unpickler.enter_symbols().unwrap();
        let labels = Labels(
            DEFINITIONS
                .iter()
                .map(|label| {
                    let at = self.unit.defs[label];
                    (*label, unpickler.index().symbol_at(at).unwrap())
                })
                .collect(),
        );
        let result = body(&mut unpickler, &labels);
        (result, labels)
    }
}

fn add_symbol(
    store: &mut SemanticStore,
    text: &str,
    namespace: Namespace,
    kind: SymbolKind,
    owner: Option<SymbolId>,
) -> SymbolId {
    let name = Name::new(store.names.intern(text), namespace);
    store.symbols.alloc(Symbol {
        name,
        owner,
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

fn reference_parts(store: &SemanticStore, ty: TypeId) -> (TypeId, SymbolId, bool) {
    match store.types.get(ty) {
        Type::TypeRef { prefix, target } => (*prefix, target.symbol().unwrap(), true),
        Type::TermRef { prefix, target } => (*prefix, target.symbol().unwrap(), false),
        other => panic!("not a reference: {other:?}"),
    }
}

fn decode(unit: &Unit, root: &str) -> (Result<TypeId, UnpickleError>, SemanticStore, Labels) {
    let mut decoder = Decoder::new(unit);
    let at = unit.roots[root];
    let (result, labels) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));
    (result, decoder.session.store, labels)
}

fn prefix_symbol(store: &SemanticStore, prefix: TypeId) -> SymbolId {
    match store.types.get(prefix) {
        ty @ Type::TypeRef { .. } => ty.reference_symbol().unwrap(),
        other => panic!("not a class reference: {other:?}"),
    }
}

// --- TYPEREFin ---

#[test]
fn a_type_reference_in_finds_the_declaration_in_the_owner_space_not_the_prefix() {
    let unit = Unit::new();

    let (result, store, symbols) = decode(&unit, "type");

    let (prefix, symbol, is_type) = reference_parts(&store, result.unwrap());
    assert!(is_type);
    // The prefix is `A`, and both `A` and `B` declare a type `T`. The owner
    // space is `B`: that one is the declaration.
    assert_eq!(symbol, symbols.get("B.T"));
    assert_ne!(symbol, symbols.get("A.T"));
    // The result keeps the original prefix, not the owner space.
    assert_eq!(prefix_symbol(&store, prefix), symbols.get("A"));
}

#[test]
fn a_term_reference_in_finds_the_declaration_in_the_owner_space_not_the_prefix() {
    let unit = Unit::new();

    let (result, store, symbols) = decode(&unit, "term");

    let (prefix, symbol, is_type) = reference_parts(&store, result.unwrap());
    assert!(!is_type);
    assert_eq!(symbol, symbols.get("B.x"));
    assert_ne!(symbol, symbols.get("A.x"));
    assert_eq!(prefix_symbol(&store, prefix), symbols.get("A"));
}

#[test]
fn the_namespace_comes_from_the_tag_not_from_the_name() {
    let unit = Unit::new();

    // `x` is a term of `B`; a `TYPEREFin` asks for a type.
    let (as_type, ..) = decode(&unit, "term named as type");
    // `T` is a type of `B`; a `TERMREFin` asks for a term.
    let (as_term, ..) = decode(&unit, "type named as term");

    assert!(matches!(
        as_type,
        Err(UnpickleError::UnresolvedMember {
            namespace: Namespace::Type,
            space: Some(_),
            ..
        })
    ));
    assert!(matches!(
        as_term,
        Err(UnpickleError::UnresolvedMember {
            namespace: Namespace::Term,
            space: Some(_),
            ..
        })
    ));
}

#[test]
fn several_declarations_in_the_owner_space_are_ambiguous_not_first_wins() {
    let unit = Unit::new();

    let (term, ..) = decode(&unit, "duplicate term");
    let (ty, ..) = decode(&unit, "duplicate type");

    assert!(matches!(
        term,
        Err(UnpickleError::AmbiguousMember {
            candidates: 2,
            space: Some(_),
            ..
        })
    ));
    assert!(matches!(
        ty,
        Err(UnpickleError::AmbiguousMember {
            candidates: 2,
            space: Some(_),
            ..
        })
    ));
}

#[test]
fn a_signed_term_reference_in_is_deferred_not_stripped() {
    let unit = Unit::new();
    let mut decoder = Decoder::new(&unit);
    let at = unit.roots["signed"];

    let (result, _) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedSignedReference {
            address: at,
            name: "x".to_owned()
        })
    );
    // The resolver is not consulted for a request that cannot be answered.
    assert!(decoder.script.borrow().asked.is_empty());
}

#[test]
fn an_owner_space_without_declaration_semantics_is_a_space_error_not_a_prefix_error() {
    let unit = Unit::new();

    let (result, ..) = decode(&unit, "alias space");

    assert!(matches!(
        result,
        Err(UnpickleError::UnsupportedResolutionSpace { .. })
    ));
}

#[test]
fn an_unstable_singleton_prefix_is_a_typed_error() {
    let unit = Unit::new();

    let (result, ..) = decode(&unit, "illegal prefix");

    assert!(matches!(
        result,
        Err(UnpickleError::IllegalTypePrefix { .. })
    ));
}

#[test]
fn a_mutable_member_prefix_is_a_typed_error_not_a_legal_prefix() {
    let unit = Unit::new();

    let (result, store, symbols) = decode(&unit, "mutable prefix");

    // The entered `var` is a `Field`, not a `Variable`: the flag decides.
    let var = symbols.get("A.v");
    assert_eq!(store.symbols.get(var).kind, SymbolKind::Field);
    assert!(matches!(
        result,
        Err(UnpickleError::IllegalTypePrefix { .. })
    ));
}

#[test]
fn a_binder_still_being_decoded_is_refused_as_prefix_and_as_space_never_read() {
    let unit = Unit::new();

    let (prefix, ..) = decode(&unit, "pending prefix");
    let (space, ..) = decode(&unit, "pending space");

    assert!(
        matches!(
            prefix,
            Err(UnpickleError::UnsupportedResolutionPrefix { .. })
        ),
        "{prefix:?}"
    );
    assert!(
        matches!(space, Err(UnpickleError::UnsupportedResolutionSpace { .. })),
        "{space:?}"
    );
}

// --- identity ---

#[test]
fn a_shared_link_to_a_reference_in_is_the_same_type_in_either_order() {
    let unit = Unit::new();
    for link_first in [false, true] {
        let mut decoder = Decoder::new(&unit);
        let (target, link) = (unit.roots["type"], unit.roots["shared type"]);

        let ((direct, linked, allocated), _) = decoder.run(|unpickler, _| {
            let (direct, linked) = if link_first {
                let linked = unpickler.unpickle_type(link).unwrap();
                (unpickler.unpickle_type(target).unwrap(), linked)
            } else {
                let direct = unpickler.unpickle_type(target).unwrap();
                (direct, unpickler.unpickle_type(link).unwrap())
            };
            (direct, linked, unpickler.index().type_count())
        });

        let Type::Flexible { underlying } = decoder.session.store.types.get(linked) else {
            panic!("a flexible wrapper expected");
        };
        assert_eq!(*underlying, direct, "link first: {link_first}");
        // prefix, space, the reference, the wrapper: the link added nothing.
        assert_eq!(allocated, 4);
    }
}

#[test]
fn a_reference_in_owns_one_type_per_address() {
    let unit = Unit::new();
    let mut decoder = Decoder::new(&unit);
    let (one, two) = (unit.roots["type"], unit.roots["type again"]);

    let ((first, again, other), _) = decoder.run(|unpickler, _| {
        let first = unpickler.unpickle_type(one).unwrap();
        (
            first,
            unpickler.unpickle_type(one).unwrap(),
            unpickler.unpickle_type(two).unwrap(),
        )
    });

    assert_eq!(first, again);
    // Equal trees at different addresses keep different ids, as for every
    // compound type.
    assert_ne!(first, other);
}

// --- the resolver ---

/// The type `Missing`, which no scope of the unit declares, as a symbol that
/// is (or is not) owned by `B`, the owner space of the `missing` root.
fn missing_answer<'u>(
    unit: &'u Unit,
    owner: Option<&str>,
    namespace: Namespace,
    kind: SymbolKind,
) -> (Decoder<'u>, SymbolId) {
    let (decoder, ids) = Decoder::with_foreign(unit, &[("Missing", namespace, kind, owner)]);
    decoder.script.borrow_mut().answer = Some(ids[0]);
    (decoder, ids[0])
}

#[test]
fn the_resolver_receives_the_original_prefix_and_the_explicit_owner_space() {
    let unit = Unit::new();
    let (mut decoder, answer) =
        missing_answer(&unit, Some("B"), Namespace::Type, SymbolKind::Class);
    let at = unit.roots["missing"];

    let (result, labels) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    let store = &decoder.session.store;
    let (prefix, symbol, _) = reference_parts(store, result.unwrap());
    assert_eq!(symbol, answer);
    // The final reference keeps the prefix `A`.
    assert_eq!(prefix_symbol(store, prefix), labels.get("A"));
    let script = decoder.script.borrow();
    let [request] = &script.asked[..] else {
        panic!("one request expected, got {}", script.asked.len());
    };
    assert_eq!(request.selector, MemberSelector::Unique);
    assert_eq!(request.name.namespace(), Namespace::Type);
    assert_eq!(request.prefix, prefix);
    let MemberSpace::Explicit(space) = request.space else {
        panic!("an explicit space expected");
    };
    // The space is the declaring owner `B`, not the prefix `A`.
    assert_ne!(space, prefix);
    assert_eq!(prefix_symbol(store, space), labels.get("B"));
}

#[test]
fn a_resolver_that_does_not_know_the_declaration_leaves_it_unresolved_without_a_prefix_retry() {
    let unit = Unit::new();
    let mut decoder = Decoder::new(&unit);
    let at = unit.roots["missing"];

    let (result, _) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedMember { space: Some(_), .. })
    ));
    // One request, for the explicit space: the prefix was never searched.
    let script = decoder.script.borrow();
    assert_eq!(script.asked.len(), 1);
    assert!(matches!(script.asked[0].space, MemberSpace::Explicit(_)));
}

#[test]
fn a_resolver_answer_from_another_owner_is_malformed_not_a_success() {
    let unit = Unit::new();
    // Same name and namespace as the request, but a member of `A`, whose
    // the prefix's class, instead of `B`, its owner space.
    let (mut decoder, _) = missing_answer(&unit, Some("A"), Namespace::Type, SymbolKind::Class);
    let at = unit.roots["missing"];

    let (result, _) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    assert!(
        matches!(
            result,
            Err(UnpickleError::ResolverFailure {
                error: ResolutionError::Malformed { .. },
                ..
            })
        ),
        "{result:?}"
    );
}

#[test]
fn a_resolver_answer_without_an_owner_is_not_a_declaration_of_the_owner_space() {
    let unit = Unit::new();
    let (mut decoder, _) = missing_answer(&unit, None, Namespace::Type, SymbolKind::Class);
    let at = unit.roots["missing"];

    let (result, _) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    assert!(matches!(
        result,
        Err(UnpickleError::ResolverFailure {
            error: ResolutionError::Malformed { .. },
            ..
        })
    ));
}

#[test]
fn a_resolver_answer_in_the_wrong_namespace_is_malformed() {
    let unit = Unit::new();
    let (mut decoder, _) = missing_answer(&unit, Some("B"), Namespace::Term, SymbolKind::Value);
    let at = unit.roots["missing"];

    let (result, _) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    assert!(matches!(
        result,
        Err(UnpickleError::ResolverFailure {
            error: ResolutionError::Malformed { .. },
            ..
        })
    ));
}

#[test]
fn a_local_hit_never_asks_the_resolver() {
    let unit = Unit::new();
    let mut decoder = Decoder::new(&unit);
    let at = unit.roots["type"];

    let (result, _) = decoder.run(|unpickler, _| unpickler.unpickle_type(at));

    assert!(result.is_ok());
    assert!(decoder.script.borrow().asked.is_empty());
}

// --- transactions ---

#[test]
fn a_failure_after_both_children_decoded_leaves_no_trace_and_a_retry_works() {
    let unit = Unit::new();
    let (mut decoder, ids) = Decoder::with_foreign(
        &unit,
        &[
            ("Missing", Namespace::Type, SymbolKind::Class, Some("A")),
            ("Missing", Namespace::Type, SymbolKind::Class, Some("B")),
        ],
    );
    let (wrong, right) = (ids[0], ids[1]);
    decoder.script.borrow_mut().answer = Some(wrong);
    let (at, other) = (unit.roots["missing"], unit.roots["type"]);
    let script = Rc::clone(&decoder.script);

    let ((kept, before, after, retried), _) = decoder.run(|unpickler, _| {
        // Pre-existing state that must survive the failure.
        let kept = unpickler.unpickle_type(other).unwrap();
        let before = (
            unpickler.index().type_count(),
            unpickler.index().type_at(at),
        );
        let failed = unpickler.unpickle_type(at);
        assert!(matches!(failed, Err(UnpickleError::ResolverFailure { .. })));
        let after = (
            unpickler.index().type_count(),
            unpickler.index().type_at(at),
        );
        assert_eq!(unpickler.index().type_at(other), Some(kept));
        script.borrow_mut().answer = Some(right);
        (kept, before, after, unpickler.unpickle_type(at))
    });

    // Both children had been decoded when the resolver's answer was refused,
    // and neither remains.
    assert_eq!(before, after);
    assert_eq!(before.1, None);
    let retried = retried.unwrap();
    assert_ne!(retried, kept);
    assert_eq!(reference_parts(&decoder.session.store, retried).1, right);
}
