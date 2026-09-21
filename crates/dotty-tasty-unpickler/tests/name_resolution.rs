//! Pass 2b: name-based `TYPEREF` / `TERMREF` resolved through the semantic
//! prefix, over the real Scala 3.9.0 `Distinct.tasty` and `Overloads.tasty`.
//!
//! The compiler writes every name-based reference in these units against a
//! package that is not entered, so most tests patch a reference that is
//! written by address (`TYPEREFsymbol`, same wire shape: a tag, a natural
//! number, a prefix) into the name-based form of the same reference. Every
//! patch first asserts what it overwrites.
//!
//! Addresses and name-table entries used in `Distinct.tasty`:
//!
//! | address | node                                                     |
//! |---------|----------------------------------------------------------|
//! | 47      | `TERMREFsymbol` to the object `Distinct`, prefix at 49   |
//! | 49      | `SHAREDtype(12)`, a `TERMREFpkg` of the unit's package   |
//! | 133     | `TYPEREFsymbol` to `Left.Inner`, prefix `THIS(Left)`     |
//! | 136     | `TYPEREFsymbol` to `Left`, prefix `THIS(Distinct$)`      |
//! | 204     | `TYPEREFsymbol` to `Right.Inner`, prefix `THIS(Right)`   |
//!
//! | name | text                                                       |
//! |------|------------------------------------------------------------|
//! | 8    | `Distinct`                                                 |
//! | 13   | signed `<init>`                                            |
//! | 32   | `Left`                                                     |
//! | 33   | `Inner`                                                    |
//! | 34   | `make`                                                     |
//! | 53   | `one`                                                      |

use std::cell::RefCell;
use std::rc::Rc;

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
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{StandardSection, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastySemanticIndex, TastyUnpickler, UnpickleError};

const DISTINCT: &[u8] = include_bytes!("fixtures/semantic/Distinct.tasty");
const OVERLOADS: &[u8] = include_bytes!("fixtures/semantic/Overloads.tasty");

const TYPEREF_TAG: u8 = 117;
const TERMREF_TAG: u8 = 115;
const TYPEREFSYMBOL_TAG: u8 = 116;
const TERMREFSYMBOL_TAG: u8 = 114;
const TYPEREFDIRECT_TAG: u8 = 63;
const TERMREFDIRECT_TAG: u8 = 62;

const OBJECT_REFERENCE: u32 = 47;
const OBJECT_PREFIX: u32 = 49;
const LEFT_INNER: u32 = 133;
const LEFT: u32 = 136;
const RIGHT_INNER: u32 = 204;

const DISTINCT_NAME: u8 = 8;
const SIGNED_INIT_NAME: u8 = 13;
const LEFT_NAME: u8 = 32;
const INNER_NAME: u8 = 33;
const MAKE_NAME: u8 = 34;
const ONE_NAME: u8 = 53;

/// Rewrites the `tag natural` head of the node at `at` to `tag` naming
/// `value` (below 128), after checking it is a `from` node. A natural of two
/// bytes is rewritten as a padded two-byte natural, which decodes the same.
fn retagged(bytes: &[u8], at: usize, from: u8, tag: u8, value: u8) -> Vec<u8> {
    assert!(value < 128);
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(payload[at], from, "unexpected node at {at}");
    let start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut patched = bytes.to_vec();
    patched[start + at] = tag;
    if payload[at + 1] & 0x80 != 0 {
        patched[start + at + 1] = 0x80 | value;
    } else {
        assert!(payload[at + 2] & 0x80 != 0, "the natural at {at} is longer");
        patched[start + at + 1] = 0;
        patched[start + at + 2] = 0x80 | value;
    }
    patched
}

/// The lowest address of a definition below 128, for a one-byte natural.
fn low_term_definition(bytes: &[u8]) -> u8 {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let index = file.ast_address_index().unwrap();
    let at = index
        .iter_nodes()
        .filter(|node| matches!(node.tag, 129 | 130) && node.offset < 128)
        .map(|node| node.offset)
        .min()
        .expect("a term definition below address 128");
    u8::try_from(at).unwrap()
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

    fn symbol(&mut self, name: &str, namespace: Namespace, kind: SymbolKind) -> SymbolId {
        let name = Name::new(self.store.names.intern(name), namespace);
        self.store.symbols.alloc(Symbol {
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
}

/// Decodes `at` in a fresh session, with `resolver` when given.
fn decode(
    bytes: &[u8],
    at: u32,
    resolver: Option<Box<dyn SymbolResolver>>,
) -> (Result<TypeId, UnpickleError>, SemanticStore) {
    let mut session = Session::new();
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    if let Some(resolver) = resolver {
        unpickler = unpickler.with_resolver(resolver);
    }
    unpickler.enter_symbols().unwrap();
    let result = unpickler.unpickle_type(at);
    drop(unpickler);
    (result, session.store)
}

fn member(store: &SemanticStore, ty: TypeId) -> (TypeId, SymbolId, bool) {
    match store.types.get(ty) {
        Type::TypeRef { prefix, symbol } => (*prefix, *symbol, true),
        Type::TermRef { prefix, symbol } => (*prefix, *symbol, false),
        other => panic!("not a reference: {other:?}"),
    }
}

fn text(store: &SemanticStore, symbol: SymbolId) -> &str {
    store.names.resolve(store.symbols.get(symbol).name.text())
}

fn owner_text(store: &SemanticStore, symbol: SymbolId) -> &str {
    text(store, store.symbols.get(symbol).owner.unwrap())
}

/// What the address-based form of the node at `at` resolved to.
fn by_address(at: u32) -> SymbolId {
    let (result, store) = decode(DISTINCT, at, None);
    member(&store, result.unwrap()).1
}

#[test]
fn a_name_based_type_reference_resolves_to_the_member_of_its_prefix() {
    let bytes = retagged(
        DISTINCT,
        LEFT_INNER as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        INNER_NAME,
    );

    let (result, store) = decode(&bytes, LEFT_INNER, None);

    let (prefix, symbol, is_type) = member(&store, result.unwrap());
    assert!(is_type);
    assert_eq!(symbol, by_address(LEFT_INNER));
    assert_eq!(text(&store, symbol), "Inner");
    assert_eq!(owner_text(&store, symbol), "Left");
    assert!(matches!(store.types.get(prefix), Type::ThisType { .. }));
}

#[test]
fn the_prefix_tells_two_same_named_members_apart() {
    let left = retagged(
        DISTINCT,
        LEFT_INNER as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        INNER_NAME,
    );
    let right = retagged(
        DISTINCT,
        RIGHT_INNER as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        INNER_NAME,
    );

    let (left_result, left_store) = decode(&left, LEFT_INNER, None);
    let (right_result, right_store) = decode(&right, RIGHT_INNER, None);

    let left_inner = member(&left_store, left_result.unwrap()).1;
    let right_inner = member(&right_store, right_result.unwrap()).1;
    // Same name text and namespace: only the prefix differs.
    assert_ne!(left_inner, right_inner);
    assert_eq!(owner_text(&left_store, left_inner), "Left");
    assert_eq!(owner_text(&right_store, right_inner), "Right");
    assert_eq!(left_inner, by_address(LEFT_INNER));
    assert_eq!(right_inner, by_address(RIGHT_INNER));
}

#[test]
fn a_name_based_type_reference_in_a_module_class_prefix_finds_the_nested_class() {
    let bytes = retagged(
        DISTINCT,
        LEFT as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        LEFT_NAME,
    );

    let (result, store) = decode(&bytes, LEFT, None);

    assert_eq!(member(&store, result.unwrap()).1, by_address(LEFT));
}

#[test]
fn this_may_name_its_class_by_name() {
    // 133 is `TYPEREFsymbol(Inner, THIS(TYPEREFsymbol(Left, ..)))`; the class
    // of the `THIS` (136) becomes the name-based `TYPEREF Left`.
    let bytes = retagged(
        DISTINCT,
        LEFT as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        LEFT_NAME,
    );

    let (result, store) = decode(&bytes, LEFT_INNER, None);

    let (prefix, _, _) = member(&store, result.unwrap());
    let Type::ThisType { class } = store.types.get(prefix) else {
        panic!("expected a ThisType prefix");
    };
    assert_eq!(*class, by_address(LEFT));
}

#[test]
fn a_name_based_term_reference_resolves_to_a_package_member() {
    let bytes = retagged(
        DISTINCT,
        OBJECT_REFERENCE as usize,
        TERMREFSYMBOL_TAG,
        TERMREF_TAG,
        DISTINCT_NAME,
    );

    let (result, store) = decode(&bytes, OBJECT_REFERENCE, None);

    let (prefix, symbol, is_type) = member(&store, result.unwrap());
    assert!(!is_type);
    assert_eq!(text(&store, symbol), "Distinct");
    assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Object);
    assert!(matches!(store.types.get(prefix), Type::TermRef { .. }));
}

#[test]
fn the_namespace_of_the_reference_selects_the_member() {
    // `Left` is a type of `Distinct$`. As a term reference the same text is
    // not found, because there is no term `Left`.
    let as_term = retagged(
        DISTINCT,
        LEFT as usize,
        TYPEREFSYMBOL_TAG,
        TERMREF_TAG,
        LEFT_NAME,
    );

    let (result, _) = decode(&as_term, LEFT, None);

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedMember {
            address: LEFT,
            ref name,
            namespace: Namespace::Term,
            ..
        }) if name == "Left"
    ));
}

#[test]
fn an_absent_member_is_unresolved_not_unsupported_and_not_guessed() {
    // `make` is a term of `Left`; there is no type of that name in it.
    let bytes = retagged(
        DISTINCT,
        LEFT_INNER as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        MAKE_NAME,
    );

    let (result, _) = decode(&bytes, LEFT_INNER, None);

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedMember {
            address: LEFT_INNER,
            ref name,
            namespace: Namespace::Type,
            ..
        }) if name == "make"
    ));
}

#[test]
fn a_signed_term_reference_is_deferred_not_stripped() {
    let bytes = retagged(
        DISTINCT,
        OBJECT_REFERENCE as usize,
        TERMREFSYMBOL_TAG,
        TERMREF_TAG,
        SIGNED_INIT_NAME,
    );

    let (result, _) = decode(&bytes, OBJECT_REFERENCE, None);

    assert_eq!(
        result,
        Err(UnpickleError::UnsupportedSignedReference {
            address: OBJECT_REFERENCE,
            name: "<init>".to_owned()
        })
    );
}

#[test]
fn overloads_are_ambiguous_and_the_first_is_never_taken() {
    // 15 is `TYPEREF name (TERMREFpkg ..)`. Its prefix (17) becomes a direct
    // reference to the class `Overloads` (definition at 4), and the node a
    // `TERMREF f`: two methods named `f`.
    let name_of_f = 18;
    let class = 4;
    let prefix = retagged(OVERLOADS, 17, 64, TYPEREFDIRECT_TAG, class);
    let bytes = retagged(&prefix, 15, TYPEREF_TAG, TERMREF_TAG, name_of_f);

    let (result, _) = decode(&bytes, 15, None);

    assert!(matches!(
        result,
        Err(UnpickleError::AmbiguousMember {
            address: 15,
            ref name,
            candidates: 2,
            ..
        }) if name == "f"
    ));
}

#[test]
fn an_object_prefix_is_searched_through_its_module_class() {
    // The prefix (49) becomes a direct term reference to the object
    // `Distinct`, and the node a `TERMREF one`: a member of `Distinct$`.
    let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(payload[OBJECT_PREFIX as usize], 61);
    let start = payload.as_ptr() as usize - DISTINCT.as_ptr() as usize;
    let mut prefix = DISTINCT.to_vec();
    prefix[start + OBJECT_PREFIX as usize] = TERMREFDIRECT_TAG;
    prefix[start + OBJECT_PREFIX as usize + 1] = 0x80 | low_term_definition(DISTINCT);
    let bytes = retagged(
        &prefix,
        OBJECT_REFERENCE as usize,
        TERMREFSYMBOL_TAG,
        TERMREF_TAG,
        ONE_NAME,
    );

    let (result, store) = decode(&bytes, OBJECT_REFERENCE, None);

    let (prefix, symbol, is_type) = member(&store, result.unwrap());
    assert!(!is_type);
    assert_eq!(text(&store, symbol), "one");
    assert_eq!(owner_text(&store, symbol), "Distinct$");
    let (_, object, _) = member(&store, prefix);
    assert_eq!(store.symbols.get(object).kind, SymbolKind::Object);
}

// --- the resolver ---

#[derive(Default)]
struct Log {
    members: Vec<MemberRequest>,
    packages: Vec<Vec<String>>,
}

struct Scripted {
    log: Rc<RefCell<Log>>,
    member: Result<Option<SymbolId>, ResolutionError>,
    package: Result<Option<SymbolId>, ResolutionError>,
}

impl SymbolResolver for Scripted {
    fn resolve_member(
        &mut self,
        _store: &SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.log.borrow_mut().members.push(request.clone());
        self.member.clone()
    }

    fn resolve_package(
        &mut self,
        _store: &SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.log
            .borrow_mut()
            .packages
            .push(path.iter().map(|s| (*s).to_owned()).collect());
        self.package.clone()
    }
}

/// `make`, as a type of `Left`, is not entered: the resolver is asked.
fn missing_member() -> Vec<u8> {
    retagged(
        DISTINCT,
        LEFT_INNER as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        MAKE_NAME,
    )
}

/// `member` says whether the resolver answers with the symbol `setup` made.
fn decode_with(
    member: Result<bool, ResolutionError>,
    setup: impl FnOnce(&mut Session) -> SymbolId,
) -> (
    Result<TypeId, UnpickleError>,
    SemanticStore,
    Rc<RefCell<Log>>,
    SymbolId,
) {
    let mut session = Session::new();
    let answer = setup(&mut session);
    let log = Rc::new(RefCell::new(Log::default()));
    let resolver = Scripted {
        log: Rc::clone(&log),
        member: member.map(|found| found.then_some(answer)),
        package: Ok(None),
    };
    let bytes = missing_member();
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions)
        .with_resolver(Box::new(resolver));
    unpickler.enter_symbols().unwrap();
    let result = unpickler.unpickle_type(LEFT_INNER);
    drop(unpickler);
    (result, session.store, log, answer)
}

#[test]
fn the_resolver_is_asked_a_semantic_question_when_the_scope_has_no_answer() {
    let (result, store, log, answer) = decode_with(Ok(true), |session| {
        session.symbol("make", Namespace::Type, SymbolKind::Class)
    });

    let (_, symbol, is_type) = member(&store, result.unwrap());
    assert!(is_type);
    assert_eq!(symbol, answer);
    let log = log.borrow();
    assert_eq!(log.members.len(), 1);
    let request = &log.members[0];
    assert_eq!(request.selector, MemberSelector::Unique);
    assert_eq!(request.space, MemberSpace::Prefix);
    assert_eq!(request.name.namespace(), Namespace::Type);
    assert_eq!(store.names.resolve(request.name.text()), "make");
    // The prefix is the decoded `THIS(Left)`, a `TypeId`, not a path.
    assert!(matches!(
        store.types.get(request.prefix),
        Type::ThisType { .. }
    ));
}

#[test]
fn a_resolver_that_does_not_know_the_member_leaves_it_unresolved() {
    let (result, _, log, _) = decode_with(Ok(false), |session| {
        session.symbol("make", Namespace::Type, SymbolKind::Class)
    });

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedMember { .. })
    ));
    assert_eq!(log.borrow().members.len(), 1);
}

#[test]
fn a_resolver_failure_is_its_own_error_and_not_not_found() {
    let failure = ResolutionError::Ambiguous { candidates: 3 };
    let (result, _, _, _) = decode_with(Err(failure.clone()), |session| {
        session.symbol("make", Namespace::Type, SymbolKind::Class)
    });

    assert_eq!(
        result,
        Err(UnpickleError::ResolverFailure {
            address: LEFT_INNER,
            error: failure
        })
    );
}

#[test]
fn a_resolver_answer_in_the_wrong_namespace_is_rejected() {
    let (result, _, _, _) = decode_with(Ok(true), |session| {
        session.symbol("make", Namespace::Term, SymbolKind::Method)
    });

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
    let mut session = Session::new();
    let log = Rc::new(RefCell::new(Log::default()));
    let resolver = Scripted {
        log: Rc::clone(&log),
        member: Ok(None),
        package: Ok(None),
    };
    let bytes = retagged(
        DISTINCT,
        LEFT_INNER as usize,
        TYPEREFSYMBOL_TAG,
        TYPEREF_TAG,
        INNER_NAME,
    );
    let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions)
        .with_resolver(Box::new(resolver));
    unpickler.enter_symbols().unwrap();

    assert!(unpickler.unpickle_type(LEFT_INNER).is_ok());

    assert!(log.borrow().members.is_empty());
}

#[test]
fn an_external_reference_resolves_through_the_package_and_member_resolver() {
    // 38 is the compiler's `TYPEREF Object (TERMREFpkg java.lang)`.
    let mut session = Session::new();
    let package = session.symbol("lang", Namespace::Term, SymbolKind::Package);
    let object = session.symbol("Object", Namespace::Type, SymbolKind::Class);
    let log = Rc::new(RefCell::new(Log::default()));
    let resolver = Scripted {
        log: Rc::clone(&log),
        member: Ok(Some(object)),
        package: Ok(Some(package)),
    };
    let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions)
        .with_resolver(Box::new(resolver));
    unpickler.enter_symbols().unwrap();

    let result = unpickler.unpickle_type(38);
    drop(unpickler);

    let (prefix, symbol, is_type) = member(&session.store, result.unwrap());
    assert!(is_type);
    assert_eq!(symbol, object);
    // The prefix is the session's canonical-prefix reference to the package.
    let (package_prefix, package_symbol, _) = member(&session.store, prefix);
    assert_eq!(package_symbol, package);
    assert_eq!(package_prefix, session.definitions.no_prefix);
    let log = log.borrow();
    assert_eq!(log.packages, [vec!["java".to_owned(), "lang".to_owned()]]);
    assert_eq!(log.members.len(), 1);
}

#[test]
fn a_package_the_resolver_does_not_know_stays_unresolved() {
    let (result, _) = decode(DISTINCT, 38, None);

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedPackage { ref package, .. }) if package == "java.lang"
    ));
}

#[test]
fn a_package_entered_by_another_adapter_is_found_without_the_resolver() {
    let mut session = Session::new();
    let mut packages = Packages::new();
    let chain = packages.enter(
        &mut session.store,
        SymbolOrigin::Synthetic,
        &["java", "lang"],
    );
    let log = Rc::new(RefCell::new(Log::default()));
    let resolver = Scripted {
        log: Rc::clone(&log),
        member: Ok(None),
        package: Ok(None),
    };
    let file = TastyFile::parse_scala_3_9(DISTINCT).unwrap();
    let mut unpickler =
        TastyUnpickler::with_packages(&file, &mut session.store, session.definitions, packages)
            .with_resolver(Box::new(resolver));
    unpickler.enter_symbols().unwrap();

    // The package resolves from the registry; `Object` is not in its scope,
    // so only the member goes to the resolver.
    let result = unpickler.unpickle_type(38);

    assert!(matches!(
        result,
        Err(UnpickleError::UnresolvedMember { .. })
    ));
    let log = log.borrow();
    assert!(log.packages.is_empty());
    assert_eq!(log.members.len(), 1);
    let _ = chain;
}

// --- transactions ---

#[test]
fn a_failed_name_based_decode_takes_back_the_prefix_it_allocated() {
    let bytes = missing_member();
    let checkpoint_of = |decode_it: bool| {
        let mut session = Session::new();
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
        unpickler.enter_symbols().unwrap();
        // A successful decode before the failing one must survive it.
        let kept = unpickler.unpickle_type(LEFT).unwrap();
        let count = unpickler.index().type_count();
        if decode_it {
            // Decodes `THIS(Left)` first, then fails at the lookup.
            assert!(unpickler.unpickle_type(LEFT_INNER).is_err());
        }
        assert_eq!(unpickler.index().type_count(), count);
        assert_eq!(unpickler.index().type_at(LEFT), Some(kept));
        let index: TastySemanticIndex = unpickler.into_index();
        (session.store.checkpoint(), index.type_count())
    };

    assert_eq!(checkpoint_of(true), checkpoint_of(false));
}
