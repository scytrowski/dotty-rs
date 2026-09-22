//! Milestone 5d2b, real fixture: `REFINEDtpt` projection against actual
//! Scala 3.9.0 output.
//!
//! `tests/fixtures/semantic/RecursiveRefined.scala` (Milestone 4a/4c2) already
//! writes every one of its structural-type parameters as a `REFINEDtpt` tree
//! (the parameter's declared type), separate from the `REFINEDtype` *type
//! node* addresses `tests/recursive.rs` already covers (Dotty writes both: the
//! declared tree and, elsewhere, a plain type-node encoding of the same
//! structural type). This file drives the eight `REFINEDtpt` trees that
//! fixture contains through `unpickle_type_tree_type`, covering every shape
//! its comments describe: a type-alias member, an upper-bound member, a
//! method member, two members folded in order, a member referring to a
//! sibling member of the same refinement, a genuine `this.type` self
//! reference, a `SHAREDtype`-shared bounds instance, and a dependent member.
//! TASTy writes *any* reference to a sibling member of the same refinement
//! as a path through the structural instance (`this.T1`, not a direct
//! symbol reference), so both the sibling-reference cases and the explicit
//! `this.type` case close over into one `Recursive`/`RecThis` — discovered
//! empirically against this fixture, not assumed from the source syntax.

use dotty_core::Name;
use dotty_core::Packages;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolOrigin;
use dotty_core::types::{
    StructuralMemberLookup, Type, TypeRefTarget, close_over_this, lookup_structural_member,
};
use dotty_core::{Definitions, SymbolFlags, SymbolInfo, SymbolKind, Visibility};
use dotty_tasty::tasty::{DEFDEF_TAG, PARAM_TAG, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

const REAL: &[u8] = include_bytes!("fixtures/semantic/RecursiveRefined.tasty");
const REFINEDTPT: u8 = 160;

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

/// A session holding the `scala.{Int,Any,Nothing}` classes the fixture
/// references but does not define, with its own symbols entered — the same
/// stub `tests/recursive.rs` uses for this fixture. A macro, not a function,
/// because `$unpickler` borrows `$file`: both must live in the caller's own
/// stack frame.
macro_rules! real_unit {
    ($file:ident, $session:ident, $unpickler:ident) => {
        let $file = TastyFile::parse_scala_3_9(REAL).unwrap();
        let mut packages = Packages::new();
        let scala = packages
            .enter(&mut $session.store, SymbolOrigin::Synthetic, &["scala"])
            .pop()
            .unwrap();
        for class in ["Int", "Any", "Nothing"] {
            let name = Name::new($session.store.names.intern(class), Namespace::Type);
            let symbol = $session.store.symbols.alloc(dotty_core::Symbol {
                name,
                owner: Some(scala.symbol),
                kind: SymbolKind::Class,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: dotty_core::SymbolLinks::default(),
            });
            $session
                .store
                .scopes
                .get_mut(scala.scope)
                .enter(name, symbol);
        }
        let mut $unpickler = TastyUnpickler::with_packages(
            &$file,
            &mut $session.store,
            $session.definitions,
            packages,
        );
        $unpickler.enter_symbols().unwrap();
    };
}

fn member_name(store: &SemanticStore, name: Name) -> String {
    store.names.resolve(name.text()).to_string()
}

fn refined_parts(
    store: &SemanticStore,
    id: dotty_core::ids::TypeId,
) -> (dotty_core::ids::TypeId, Name, dotty_core::ids::TypeId) {
    match store.types.get(id) {
        Type::Refined { parent, name, info } => (*parent, *name, *info),
        other => panic!("not a refined type: {other:?}"),
    }
}

/// The address of the `REFINEDtpt` that is the sole parameter's declared
/// type of the `DEFDEF` named `method`.
fn refined_tpt_at(file: &TastyFile<'_>, method: &str) -> u32 {
    let index = file.ast_address_index().unwrap();
    let defdef = index
        .iter()
        .find(|node| {
            node.tag == DEFDEF_TAG
                && node.decode_definition().is_ok_and(|def| {
                    file.names()
                        .get(def.name())
                        .is_some_and(|name| format!("{name:?}").contains(&format!("\"{method}\"")))
                })
        })
        .unwrap();
    let defdef_at = defdef.offset;
    let param = index
        .iter_tree_edges()
        .find(|edge| edge.parent.offset == defdef_at && edge.child.tag == PARAM_TAG)
        .unwrap();
    index
        .iter_tree_edges()
        .find(|edge| edge.parent.offset == param.child.offset && edge.child.tag == REFINEDTPT)
        .map(|edge| u32::try_from(edge.child.offset).unwrap())
        .unwrap()
}

#[test]
fn a_real_type_alias_member_is_a_refined_chain_with_the_projected_parent() {
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "typeMember");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let (parent, name, info) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, name), "T");
    assert_eq!(name.namespace(), Namespace::Type);
    assert!(matches!(
        session.store.types.get(info),
        Type::AliasingBounds { .. }
    ));
    assert!(matches!(
        session.store.types.get(parent),
        Type::TypeRef { .. }
    ));
}

#[test]
fn a_real_upper_bound_member_has_two_sided_bounds() {
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "upperMember");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let (_, name, info) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, name), "T");
    assert!(matches!(session.store.types.get(info), Type::Bounds { .. }));
}

#[test]
fn a_real_method_member_has_a_term_name_and_a_method_info() {
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "methodMember");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let (_, name, info) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, name), "run");
    assert_eq!(name.namespace(), Namespace::Term);
    assert!(matches!(session.store.types.get(info), Type::Method(_)));
}

#[test]
fn real_members_fold_into_the_chain_in_wire_order() {
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "twoMembers");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    // `{ type T = Int; def run(...): Int }`: T is written first, so it is
    // the *inner* (parent-most) refinement, and `run` the outer one.
    let (inner, outer_name, _) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, outer_name), "run");
    let (_, inner_name, _) = refined_parts(&session.store, inner);
    assert_eq!(member_name(&session.store, inner_name), "T");
}

#[test]
fn a_member_naming_a_sibling_closes_over_into_one_recursive() {
    // `C { type T1; type T2 = T1 }`: TASTy writes a reference to a sibling
    // member of the same refinement as a path through the structural
    // instance (`this.T1`), so T2's alias is a name-designated `TypeRef`
    // whose prefix is the synthetic class's `ThisType` — closed over into
    // one `Recursive`/`RecThis`, the same as an explicit `this.type` self
    // reference.
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "recursive");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let Type::Recursive { parent } = session.store.types.get(id) else {
        panic!("expected Recursive, got {:?}", session.store.types.get(id));
    };
    let (inner, outer_name, outer_info) = refined_parts(&session.store, *parent);
    assert_eq!(member_name(&session.store, outer_name), "T2");
    let (_, inner_name, _) = refined_parts(&session.store, inner);
    assert_eq!(member_name(&session.store, inner_name), "T1");
    let Type::AliasingBounds { alias } = session.store.types.get(outer_info) else {
        panic!("expected alias bounds");
    };
    // T1's reference carries a real prefix (`TYPEREFsymbol`, not the
    // prefix-less `TYPEREFdirect`): the entered T1 symbol, reached through
    // the refinement's own `this`.
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(_),
    } = session.store.types.get(*alias)
    else {
        panic!(
            "expected a symbol-designated type ref, got {:?}",
            session.store.types.get(*alias)
        );
    };
    assert_eq!(
        session.store.types.get(*prefix),
        &Type::RecThis { binder: id }
    );
}

#[test]
fn a_real_this_type_self_reference_closes_over_into_one_recursive() {
    // `Base { def me: this.type }`: `this` inside the refinement names the
    // synthetic refinement class, closed over into one Recursive/RecThis.
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "selfType");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let Type::Recursive { parent } = session.store.types.get(id) else {
        panic!("expected Recursive, got {:?}", session.store.types.get(id));
    };
    let (_, name, info) = refined_parts(&session.store, *parent);
    assert_eq!(member_name(&session.store, name), "me");
    assert_eq!(name.namespace(), Namespace::Term);
    // `def me: this.type` has no parameter clause, so its info is the
    // paramless `ByName` shape (Milestone 5c's "no clause" rule), wrapping
    // the canonical `RecThis` of this very `Recursive`.
    let Type::ByName { result } = session.store.types.get(info) else {
        panic!(
            "expected a by-name info, got {:?}",
            session.store.types.get(info)
        );
    };
    assert_eq!(
        session.store.types.get(*result),
        &Type::RecThis { binder: id }
    );
}

#[test]
fn a_real_shared_bounds_link_still_carries_its_own_member_name() {
    // `Base { type T = Int; type U = Int }`: T and U's `Int` bounds are the
    // very same `AliasingBounds` instance (a `SHAREDtype` link on the wire,
    // Dotty deduplicating the repeated `Int` alias within this refinement),
    // even though they name different members.
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "sharedBounds");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let (inner, outer_name, outer_info) = refined_parts(&session.store, id);
    assert_eq!(member_name(&session.store, outer_name), "U");
    let (_, inner_name, inner_info) = refined_parts(&session.store, inner);
    assert_eq!(member_name(&session.store, inner_name), "T");
    // Each `TYPEDEF` completion wraps its type in its own fresh
    // `AliasingBounds` (`bounds_of`), so the two members' *infos* are
    // distinct allocations; what the wire actually shares is the `Int`
    // reference underneath both.
    let Type::AliasingBounds { alias: outer_alias } = session.store.types.get(outer_info) else {
        panic!("expected alias bounds");
    };
    let Type::AliasingBounds { alias: inner_alias } = session.store.types.get(inner_info) else {
        panic!("expected alias bounds");
    };
    assert_ne!(outer_info, inner_info);
    assert_eq!(outer_alias, inner_alias);
}

#[test]
fn a_real_dependent_member_closes_over_into_one_recursive() {
    // `Base { type T = Int; type U = T }`: like `recursive`, U's reference to
    // T goes through the structural instance's `this`, so the chain closes
    // over into one `Recursive`.
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "dependent");

    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let Type::Recursive { parent } = session.store.types.get(id) else {
        panic!("expected Recursive, got {:?}", session.store.types.get(id));
    };
    let (_, name, info) = refined_parts(&session.store, *parent);
    assert_eq!(member_name(&session.store, name), "U");
    let Type::AliasingBounds { alias } = session.store.types.get(info) else {
        panic!("expected alias bounds");
    };
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(_),
    } = session.store.types.get(*alias)
    else {
        panic!(
            "expected a symbol-designated type ref, got {:?}",
            session.store.types.get(*alias)
        );
    };
    assert_eq!(
        session.store.types.get(*prefix),
        &Type::RecThis { binder: id }
    );
}

#[test]
fn projecting_a_real_refined_tpt_twice_is_the_same_id_and_allocates_nothing_more() {
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "typeMember");

    let first = unpickler.unpickle_type_tree_type(at).unwrap();
    let types = unpickler.index().type_count();
    let second = unpickler.unpickle_type_tree_type(at).unwrap();

    assert_eq!(first, second);
    assert_eq!(unpickler.index().type_count(), types);
}

#[test]
fn structural_lookup_finds_a_real_projected_members_info() {
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "twoMembers");
    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    drop(unpickler);

    let t = Name::new(session.store.names.intern("T"), Namespace::Type);
    let run = Name::new(session.store.names.intern("run"), Namespace::Term);
    assert!(matches!(
        lookup_structural_member(&session.store, id, t),
        Ok(StructuralMemberLookup::Found { .. })
    ));
    assert!(matches!(
        lookup_structural_member(&session.store, id, run),
        Ok(StructuralMemberLookup::Found { .. })
    ));
    let missing = Name::new(session.store.names.intern("nope"), Namespace::Type);
    assert_eq!(
        lookup_structural_member(&session.store, id, missing),
        Ok(StructuralMemberLookup::NotFound)
    );
}

#[test]
fn no_synthetic_refinement_class_this_type_leaks_into_a_real_closed_graph() {
    // After close_over_this, every recursive occurrence is a RecThis; a
    // second, independent close_over_this call over the already-closed graph
    // finds nothing left to close (no ThisType(refine_cls) remains).
    let mut session = Session::new();
    real_unit!(file, session, unpickler);
    let at = refined_tpt_at(&file, "selfType");
    let id = unpickler.unpickle_type_tree_type(at).unwrap();
    let refine_cls = unpickler.index().symbol_at(at).unwrap();
    drop(unpickler);

    let again = close_over_this(&mut session.store, id, refine_cls).unwrap();
    assert_eq!(again, id);
}
