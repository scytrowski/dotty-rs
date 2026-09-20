//! Milestone 2c2: `TYPEBOUNDS`, over the real Scala 3.9.0 `Bounds.tasty`.
use dotty_core::Definitions;
use dotty_core::ids::TypeId;
use dotty_core::store::SemanticStore;
use dotty_core::types::Type;
use dotty_tasty::tasty::{StandardSection, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const BOUNDS: &[u8] = include_bytes!("fixtures/semantic/Bounds.tasty");

fn show(store: &SemanticStore, id: TypeId) -> String {
    let name = |symbol| {
        let symbol = store.symbols.get(symbol);
        store.names.resolve(symbol.name.text()).to_string()
    };
    match store.types.get(id) {
        Type::TypeRef { symbol, .. } => name(*symbol),
        Type::Applied { tycon, args } => {
            let args: Vec<_> = args.iter().map(|arg| show(store, *arg)).collect();
            format!("{}[{}]", show(store, *tycon), args.join(", "))
        }
        Type::Bounds { low, high } => {
            format!(">: {} <: {}", show(store, *low), show(store, *high))
        }
        Type::AliasingBounds { alias } => format!("= {}", show(store, *alias)),
        other => format!("{other:?}"),
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

/// `>: High <: Low`, a real two-sided `TYPEBOUNDS` whose children are
/// `SHAREDtype` links at 196 and 199.
const TWO_SIDED: u32 = 194;
/// `TYPEBOUNDS` with only an alias child, `= Item`; the child is at 432.
const ALIAS: u32 = 430;
const ALIAS_CHILD: usize = 432;
/// Alias bounds over `List[Item]`, whose constructor lives in a package the
/// unit does not enter.
const ALIAS_EXTERNAL: u32 = 468;
/// A real `SHAREDtype` link to [`TWO_SIDED`].
const SHARED_TWO_SIDED: u32 = 364;
const HIGH_CHILD: usize = 199;

fn open(bytes: &[u8]) -> (TastyFile<'_>, Session) {
    (TastyFile::parse_scala_3_9(bytes).unwrap(), Session::new())
}

fn retagged(bytes: &[u8], at: usize, from: u8, to: u8) -> Vec<u8> {
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    assert_eq!(
        payload[at], from,
        "the fixture no longer has {from} at {at}"
    );
    let start = payload.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut patched = bytes.to_vec();
    patched[start + at] = to;
    patched
}

#[test]
fn two_sided_bounds_keep_their_low_and_high() {
    let (file, mut session) = open(BOUNDS);
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(TWO_SIDED).unwrap();
    drop(unpickler);

    assert_eq!(show(&session.store, id), ">: High <: Low");
    assert!(matches!(session.store.types.get(id), Type::Bounds { .. }));
}

#[test]
fn alias_only_bounds_are_aliasing_bounds_not_equal_ends() {
    let (file, mut session) = open(BOUNDS);
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(ALIAS).unwrap();
    drop(unpickler);

    let Type::AliasingBounds { alias } = session.store.types.get(id) else {
        panic!(
            "expected AliasingBounds, got {:?}",
            session.store.types.get(id)
        );
    };
    assert_eq!(show(&session.store, *alias), "Item");
}

#[test]
fn decoding_bounds_twice_returns_the_same_type_id() {
    let (file, mut session) = open(BOUNDS);
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    for at in [TWO_SIDED, ALIAS] {
        let first = unpickler.unpickle_type(at).unwrap();
        assert_eq!(unpickler.unpickle_type(at), Ok(first));
    }
}

#[test]
fn a_shared_type_reuses_the_id_of_the_bounds_it_points_at() {
    let (file, mut session) = open(BOUNDS);
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let shared = unpickler.unpickle_type(SHARED_TWO_SIDED).unwrap();
    assert_eq!(unpickler.unpickle_type(TWO_SIDED), Ok(shared));
    assert_eq!(unpickler.index().type_at(SHARED_TWO_SIDED), None);
}

#[test]
fn bounds_children_resolve_through_shared_links_to_the_same_ids() {
    let (file, mut session) = open(BOUNDS);
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let id = unpickler.unpickle_type(TWO_SIDED).unwrap();
    let low = unpickler.unpickle_type(196).unwrap();
    let high = unpickler.unpickle_type(199).unwrap();
    drop(unpickler);

    assert_eq!(session.store.types.get(id), &Type::Bounds { low, high });
    assert_ne!(low, high);
}

#[test]
fn an_unresolved_alias_child_is_reported_as_itself_and_rolls_back() {
    let (file, mut session) = open(BOUNDS);
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(ALIAS_EXTERNAL),
        Err(UnpickleError::UnresolvedPackage { .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(ALIAS_EXTERNAL), None);
}

#[test]
fn a_failing_alias_child_leaves_no_trace() {
    let patched = retagged(BOUNDS, ALIAS_CHILD, 61, 75);
    let file = TastyFile::parse_scala_3_9(&patched).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(ALIAS),
        Err(UnpickleError::UnsupportedType { tag: 75, .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(ALIAS), None);
}

#[test]
fn a_failing_high_bound_forgets_the_low_bound_that_decoded() {
    let patched = retagged(BOUNDS, HIGH_CHILD, 61, 75);
    let file = TastyFile::parse_scala_3_9(&patched).unwrap();
    let mut session = Session::new();
    let mut unpickler = TastyUnpickler::new(&file, &mut session.store, session.definitions);
    unpickler.enter_symbols().unwrap();
    let before = unpickler.index().type_count();

    assert!(matches!(
        unpickler.unpickle_type(TWO_SIDED),
        Err(UnpickleError::UnsupportedType { tag: 75, .. })
    ));
    assert_eq!(unpickler.index().type_count(), before);
    assert_eq!(unpickler.index().type_at(196), None);
    assert_eq!(unpickler.index().type_at(TWO_SIDED), None);
}
