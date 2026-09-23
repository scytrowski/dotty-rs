//! The address-keyed semantic index.
//!
//! TASTy AST addresses are the identity anchors of the format: a definition
//! is referred to by the address of its node, never by name. The index maps
//! each such address to the one `dotty-core` value that represents it, so a
//! later reference to the same address resolves to the same identity instead
//! of building a second one.

use std::collections::{HashMap, HashSet};

use dotty_core::ids::{ScopeId, SymbolId, TypeId};

use crate::discovery::{DiscoveryMode, DiscoveryRoute};
use crate::error::UnpickleError;

/// Maps TASTy definition addresses to the semantic entities entered for them.
///
/// The invariant is that one definition address owns exactly one
/// [`SymbolId`]. Addresses are absolute offsets in the AST section, as
/// reported by `dotty-tasty`'s address index.
///
/// Declaration scopes are keyed by the *owning symbol* rather than by an
/// address: a package can be spread over several `PACKAGE` nodes that share
/// one symbol and one scope, so no single node address identifies it.
///
/// Type identity follows the same rule: one semantic type node address owns
/// at most one [`TypeId`]. This is address identity, not structural
/// interning: two separately written, structurally equal type trees have
/// different addresses and may have different `TypeId`s. A `SHAREDtype` node
/// is an indirection and has no entry of its own; it resolves to the entry of
/// the node it names. A map for typed trees is added by the milestone that
/// populates it.
#[derive(Debug, Default, Clone)]
pub struct TastySemanticIndex {
    symbols: HashMap<u32, SymbolId>,
    scopes: HashMap<SymbolId, ScopeId>,
    types: HashMap<u32, TypeId>,
    /// Type addresses in the order they were recorded, so a failed type
    /// operation can forget the newest ones.
    type_order: Vec<u32>,
    /// The semantic type of the *type tree* at an address (Milestone 5a).
    /// Kept apart from `types`: a tree address is not a type-node address,
    /// several tree addresses may project to one `TypeId` (an `IDENTtpt` has
    /// exactly its embedded type), and a derived type (`APPLIEDtpt`) is owned
    /// by the projection, not by any type node.
    type_trees: HashMap<u32, TypeId>,
    /// Tree addresses in recording order, to roll back.
    type_tree_order: Vec<u32>,
    /// The semantic `tpe` of the *term tree* at an address (Milestone 5b), for
    /// the paths a `SELECTtpt` qualifier or `SINGLETONtpt` reference is made of.
    /// Apart from both maps above: several term addresses may denote one
    /// `TypeId`, and a `SHAREDterm` has no entry of its own.
    term_trees: HashMap<u32, TypeId>,
    /// Term addresses in recording order, to roll back.
    term_tree_order: Vec<u32>,
    /// The owner each `LAMBDAtpt` entered its type parameters for (Milestone
    /// 5c, pass 1): the first semantic owner to reach the tree.
    lambda_owners: HashMap<u32, SymbolId>,
    /// `LAMBDAtpt` addresses reached again, through a `SHAREDterm`, from a
    /// different owner than the one that entered them: their parameters are
    /// owned once, so projecting them is refused (`SharedLambdaOwnerConflict`).
    lambda_conflicts: HashSet<u32>,
    /// The structural hop that led discovery to each `LAMBDAtpt`'s *first*
    /// owner (Milestone 5d2c's follow-up review, route-attribution metrics):
    /// see [`DiscoveryRoute`](crate::discovery::DiscoveryRoute).
    lambda_routes: HashMap<u32, DiscoveryRoute>,
    /// `(tree, owner, mode)` triples the identity-discovery walker has
    /// visited (Milestone 5d2c; `LAMBDAtpt` type parameters, Milestone 5c;
    /// `REFINEDtpt` synthetic classes, Milestone 5d2b), so a tree shared by
    /// many definitions, or reached under several structural interpretations,
    /// is walked once per `(owner, mode)` pair, not once per path. The mode
    /// is part of the key precisely so an earlier shallow visit (say,
    /// `SemanticType`, which does not follow a direct/symbol reference's
    /// target) cannot suppress a later, semantically richer one (`ClassRef`,
    /// which does) of the very same address: see
    /// [`DiscoveryMode`](crate::discovery::DiscoveryMode).
    identity_scans: HashSet<(u32, SymbolId, DiscoveryMode)>,
    /// The owner each `REFINEDtpt` entered its synthetic refinement class
    /// for (Milestone 5d2b, pass 1): the first semantic owner to reach the
    /// tree. The class itself is entered through the ordinary `symbols` map,
    /// keyed by the `REFINEDtpt` address like every other definition; this is
    /// only the owner-conflict bookkeeping, mirroring `lambda_owners`.
    refined_owners: HashMap<u32, SymbolId>,
    /// `REFINEDtpt` addresses reached again, through a `SHAREDterm`, from a
    /// different owner than the one that entered them
    /// (`SharedRefinementOwnerConflict`), mirroring `lambda_conflicts`.
    refined_conflicts: HashSet<u32>,
    /// The structural hop that led discovery to each `REFINEDtpt`'s *first*
    /// owner, mirroring `lambda_routes`.
    refined_routes: HashMap<u32, DiscoveryRoute>,
    /// The `ANNOTATION` tail entries of the definition/parameter at an
    /// address, in wire order, recorded when its symbol is entered
    /// (Milestone 5e1, pass 1). An address with no `ANNOTATION` entries is
    /// still present, mapped to an empty `Vec`, so a later completion pass
    /// can tell "no annotations in the wire" apart from "no symbol was ever
    /// entered here" — see [`annotation_tail_at`](Self::annotation_tail_at).
    /// No decoding happens here: each value is only the ordered addresses of
    /// the `ANNOTATION` nodes themselves.
    annotation_tails: HashMap<u32, Vec<u32>>,
}

impl TastySemanticIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// The symbol entered for the definition at `address`, if any.
    pub fn symbol_at(&self, address: u32) -> Option<SymbolId> {
        self.symbols.get(&address).copied()
    }

    /// The declaration scope of `symbol`, for symbols that own one (packages
    /// and classes).
    pub fn scope_of(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.scopes.get(&symbol).copied()
    }

    /// The type decoded for the type node at `address`, if any.
    pub fn type_at(&self, address: u32) -> Option<TypeId> {
        self.types.get(&address).copied()
    }

    /// The semantic type projected for the type tree at `address`, if any.
    /// Not a [`type_at`](Self::type_at): that is keyed by type-node address.
    pub fn type_tree_type_at(&self, address: u32) -> Option<TypeId> {
        self.type_trees.get(&address).copied()
    }

    /// The symbol that owns the type parameters entered for the `LAMBDAtpt` at
    /// `address`, if it was reached in a declared type-tree position.
    pub fn lambda_owner(&self, address: u32) -> Option<SymbolId> {
        self.lambda_owners.get(&address).copied()
    }

    /// Whether the `LAMBDAtpt` at `address` was reached from more than one
    /// owner (a `SHAREDterm` shared across definitions).
    pub fn has_lambda_owner_conflict(&self, address: u32) -> bool {
        self.lambda_conflicts.contains(&address)
    }

    /// The symbol that owns the synthetic refinement class entered for the
    /// `REFINEDtpt` at `address`, if it was reached in a declared type-tree
    /// position.
    pub fn refined_owner(&self, address: u32) -> Option<SymbolId> {
        self.refined_owners.get(&address).copied()
    }

    /// Whether the `REFINEDtpt` at `address` was reached from more than one
    /// owner (a `SHAREDterm` shared across definitions).
    pub fn has_refined_owner_conflict(&self, address: u32) -> bool {
        self.refined_conflicts.contains(&address)
    }

    /// The semantic type projected for the term tree at `address`, if any.
    pub fn term_tree_type_at(&self, address: u32) -> Option<TypeId> {
        self.term_trees.get(&address).copied()
    }

    /// The number of term-tree addresses that have a projected type.
    pub fn term_tree_count(&self) -> usize {
        self.term_trees.len()
    }

    /// The number of type-tree addresses that have a projected type.
    pub fn type_tree_count(&self) -> usize {
        self.type_trees.len()
    }

    /// The number of type addresses that have a `TypeId`.
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    /// The number of definition addresses that have a symbol.
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    /// Records the symbol for a definition address.
    ///
    /// A second symbol for an address that already has one would break the
    /// address-identity invariant, so it is rejected and the existing entry
    /// is kept.
    pub(crate) fn insert_symbol(
        &mut self,
        address: u32,
        symbol: SymbolId,
    ) -> Result<(), UnpickleError> {
        match self.symbols.entry(address) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(UnpickleError::DuplicateDefinition { address })
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(symbol);
                Ok(())
            }
        }
    }

    /// Records the type for a type-node address.
    ///
    /// Like [`insert_symbol`](Self::insert_symbol), a second `TypeId` for an
    /// address is rejected and the existing entry is kept.
    pub(crate) fn insert_type(&mut self, address: u32, ty: TypeId) -> Result<(), UnpickleError> {
        match self.types.entry(address) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(UnpickleError::DuplicateType { address })
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ty);
                self.type_order.push(address);
                Ok(())
            }
        }
    }

    /// Records the type projected for a type-tree address; a second one for
    /// the address is rejected and the existing entry kept.
    pub(crate) fn insert_type_tree(
        &mut self,
        address: u32,
        ty: TypeId,
    ) -> Result<(), UnpickleError> {
        match self.type_trees.entry(address) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(UnpickleError::DuplicateType { address })
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ty);
                self.type_tree_order.push(address);
                Ok(())
            }
        }
    }

    pub(crate) fn insert_lambda_owner(&mut self, address: u32, owner: SymbolId) {
        self.lambda_owners.insert(address, owner);
    }

    pub(crate) fn mark_lambda_conflict(&mut self, address: u32) {
        self.lambda_conflicts.insert(address);
    }

    pub(crate) fn insert_lambda_route(&mut self, address: u32, route: DiscoveryRoute) {
        self.lambda_routes.insert(address, route);
    }

    /// The name of the structural hop that led discovery to the `LAMBDAtpt`
    /// at `address`, for the corpus route-attribution report; `None` when the
    /// address was never entered.
    pub fn lambda_route(&self, address: u32) -> Option<&'static str> {
        self.lambda_routes.get(&address).map(|route| route.name())
    }

    pub(crate) fn insert_refined_owner(&mut self, address: u32, owner: SymbolId) {
        self.refined_owners.insert(address, owner);
    }

    pub(crate) fn mark_refined_conflict(&mut self, address: u32) {
        self.refined_conflicts.insert(address);
    }

    pub(crate) fn insert_refined_route(&mut self, address: u32, route: DiscoveryRoute) {
        self.refined_routes.insert(address, route);
    }

    /// The name of the structural hop that led discovery to the `REFINEDtpt`
    /// at `address`, mirroring [`lambda_route`](Self::lambda_route).
    pub fn refined_route(&self, address: u32) -> Option<&'static str> {
        self.refined_routes.get(&address).map(|route| route.name())
    }

    /// Whether `(tree, owner, mode)` had not been visited by the identity
    /// walker before; records it.
    pub(crate) fn first_identity_scan(
        &mut self,
        tree: u32,
        owner: SymbolId,
        mode: DiscoveryMode,
    ) -> bool {
        self.identity_scans.insert((tree, owner, mode))
    }

    /// Records the type projected for a term-tree address; a second one for
    /// the address is rejected and the existing entry kept.
    pub(crate) fn insert_term_tree(
        &mut self,
        address: u32,
        ty: TypeId,
    ) -> Result<(), UnpickleError> {
        match self.term_trees.entry(address) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(UnpickleError::DuplicateType { address })
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ty);
                self.term_tree_order.push(address);
                Ok(())
            }
        }
    }

    /// A position to [`roll_back_term_trees`](Self::roll_back_term_trees).
    pub(crate) fn mark_term_trees(&self) -> usize {
        self.term_tree_order.len()
    }

    /// Forgets every term-tree address recorded after `mark`.
    pub(crate) fn roll_back_term_trees(&mut self, mark: usize) {
        while self.term_tree_order.len() > mark {
            if let Some(address) = self.term_tree_order.pop() {
                self.term_trees.remove(&address);
            }
        }
    }

    /// A position to [`roll_back_type_trees`](Self::roll_back_type_trees).
    pub(crate) fn mark_type_trees(&self) -> usize {
        self.type_tree_order.len()
    }

    /// Forgets every type-tree address recorded after `mark`.
    pub(crate) fn roll_back_type_trees(&mut self, mark: usize) {
        while self.type_tree_order.len() > mark {
            if let Some(address) = self.type_tree_order.pop() {
                self.type_trees.remove(&address);
            }
        }
    }

    /// A position to [`roll_back_types`](Self::roll_back_types).
    pub(crate) fn mark_types(&self) -> usize {
        self.type_order.len()
    }

    /// Forgets every type address recorded after `mark`.
    pub(crate) fn roll_back_types(&mut self, mark: usize) {
        while self.type_order.len() > mark {
            if let Some(address) = self.type_order.pop() {
                self.types.remove(&address);
            }
        }
    }

    /// The `ANNOTATION` tail addresses recorded for the definition/parameter
    /// at `address`, if a symbol was entered for it: `Some(&[])` when it was
    /// entered with no `ANNOTATION` entries, `None` when no symbol was ever
    /// entered at `address`.
    pub fn annotation_tail_at(&self, address: u32) -> Option<&[u32]> {
        self.annotation_tails.get(&address).map(Vec::as_slice)
    }

    /// The number of definition/parameter addresses that carry at least one
    /// `ANNOTATION` tail entry.
    pub fn annotated_definition_count(&self) -> usize {
        self.annotation_tails
            .values()
            .filter(|entries| !entries.is_empty())
            .count()
    }

    /// The total number of `ANNOTATION` tail entries recorded across every
    /// definition/parameter address.
    pub fn annotation_tail_count(&self) -> usize {
        self.annotation_tails.values().map(Vec::len).sum()
    }

    /// Records the `ANNOTATION` tail addresses for the definition/parameter
    /// at `address`, in wire order. Called once per address, right after its
    /// symbol is entered; a second call for the same address would silently
    /// overwrite the first; pass 1 never does that, since
    /// [`insert_symbol`](Self::insert_symbol) already rejects a second entry
    /// at the same address before this would ever run again.
    pub(crate) fn insert_annotation_tail(&mut self, address: u32, annotations: Vec<u32>) {
        self.annotation_tails.insert(address, annotations);
    }

    /// Records the scope of a package that another unit entered, for the
    /// unit to find with `scope_of`. Recording it twice is not an error.
    pub(crate) fn share_scope(&mut self, symbol: SymbolId, scope: ScopeId) {
        self.scopes.entry(symbol).or_insert(scope);
    }

    /// Records the declaration scope owned by `symbol`.
    ///
    /// A symbol has at most one declaration scope; a second one is rejected
    /// and the existing entry is kept.
    pub(crate) fn insert_scope(
        &mut self,
        symbol: SymbolId,
        scope: ScopeId,
    ) -> Result<(), UnpickleError> {
        match self.scopes.entry(symbol) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(UnpickleError::DuplicateScope { symbol })
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(scope);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::names::{Name, Namespace};
    use dotty_core::store::SemanticStore;
    use dotty_core::symbols::{
        Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };

    /// Real IDs can only be minted by `dotty-core`'s arenas.
    fn allocate_symbol(store: &mut SemanticStore, text: &str) -> SymbolId {
        let name = Name::new(store.names.intern(text), Namespace::Type);
        store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    #[test]
    fn a_new_index_knows_no_definitions() {
        let index = TastySemanticIndex::new();

        assert_eq!(index.symbol_at(4), None);
        assert_eq!(index.symbol_count(), 0);
    }

    #[test]
    fn an_inserted_address_resolves_to_its_symbol() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let mut index = TastySemanticIndex::new();

        index.insert_symbol(4, foo).unwrap();

        assert_eq!(index.symbol_at(4), Some(foo));
    }

    #[test]
    fn repeated_lookup_of_one_address_returns_the_same_symbol() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let mut index = TastySemanticIndex::new();
        index.insert_symbol(4, foo).unwrap();

        assert_eq!(index.symbol_at(4), index.symbol_at(4));
        assert_eq!(index.symbol_count(), 1);
    }

    #[test]
    fn distinct_addresses_keep_distinct_symbols() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let bar = allocate_symbol(&mut store, "Bar");
        let mut index = TastySemanticIndex::new();

        index.insert_symbol(4, foo).unwrap();
        index.insert_symbol(46, bar).unwrap();

        assert_eq!(index.symbol_at(4), Some(foo));
        assert_eq!(index.symbol_at(46), Some(bar));
        assert_eq!(index.symbol_count(), 2);
    }

    #[test]
    fn a_second_symbol_for_an_address_is_rejected_and_keeps_the_first() {
        let mut store = SemanticStore::new();
        let first = allocate_symbol(&mut store, "First");
        let second = allocate_symbol(&mut store, "Second");
        let mut index = TastySemanticIndex::new();
        index.insert_symbol(4, first).unwrap();

        let result = index.insert_symbol(4, second);

        assert_eq!(
            result,
            Err(UnpickleError::DuplicateDefinition { address: 4 })
        );
        assert_eq!(index.symbol_at(4), Some(first));
    }

    #[test]
    fn re_inserting_the_same_symbol_at_its_address_is_also_rejected() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let mut index = TastySemanticIndex::new();
        index.insert_symbol(4, foo).unwrap();

        let result = index.insert_symbol(4, foo);

        assert_eq!(
            result,
            Err(UnpickleError::DuplicateDefinition { address: 4 })
        );
    }

    fn allocate_type(store: &mut SemanticStore) -> TypeId {
        store.types.alloc(dotty_core::types::Type::NoPrefix)
    }

    #[test]
    fn a_new_index_knows_no_types() {
        let index = TastySemanticIndex::new();

        assert_eq!(index.type_at(4), None);
        assert_eq!(index.type_count(), 0);
    }

    #[test]
    fn an_inserted_type_address_resolves_to_its_type() {
        let mut store = SemanticStore::new();
        let ty = allocate_type(&mut store);
        let mut index = TastySemanticIndex::new();

        index.insert_type(4, ty).unwrap();

        assert_eq!(index.type_at(4), Some(ty));
        assert_eq!(index.type_count(), 1);
    }

    #[test]
    fn distinct_type_addresses_keep_distinct_types() {
        let mut store = SemanticStore::new();
        let (first, second) = (allocate_type(&mut store), allocate_type(&mut store));
        let mut index = TastySemanticIndex::new();

        index.insert_type(4, first).unwrap();
        index.insert_type(9, second).unwrap();

        assert_eq!(index.type_at(4), Some(first));
        assert_eq!(index.type_at(9), Some(second));
    }

    #[test]
    fn a_second_type_for_an_address_is_rejected_and_keeps_the_first() {
        let mut store = SemanticStore::new();
        let (first, second) = (allocate_type(&mut store), allocate_type(&mut store));
        let mut index = TastySemanticIndex::new();
        index.insert_type(4, first).unwrap();

        assert_eq!(
            index.insert_type(4, second),
            Err(UnpickleError::DuplicateType { address: 4 })
        );
        assert_eq!(
            index.insert_type(4, first),
            Err(UnpickleError::DuplicateType { address: 4 })
        );
        assert_eq!(index.type_at(4), Some(first));
        assert_eq!(index.type_count(), 1);
    }

    #[test]
    fn rolling_back_forgets_only_the_types_recorded_after_the_mark() {
        let mut store = SemanticStore::new();
        let (first, second) = (allocate_type(&mut store), allocate_type(&mut store));
        let mut index = TastySemanticIndex::new();
        index.insert_type(4, first).unwrap();
        let mark = index.mark_types();
        index.insert_type(9, second).unwrap();

        index.roll_back_types(mark);

        assert_eq!(index.type_at(4), Some(first));
        assert_eq!(index.type_at(9), None);
    }

    #[test]
    fn a_symbol_without_a_scope_has_none() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let index = TastySemanticIndex::new();

        assert_eq!(index.scope_of(foo), None);
    }

    #[test]
    fn an_inserted_scope_resolves_through_its_owning_symbol() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let scope = store.scopes.alloc(Scope::new(Some(foo)));
        let mut index = TastySemanticIndex::new();

        index.insert_scope(foo, scope).unwrap();

        assert_eq!(index.scope_of(foo), Some(scope));
    }

    #[test]
    fn a_second_scope_for_a_symbol_is_rejected_and_keeps_the_first() {
        let mut store = SemanticStore::new();
        let foo = allocate_symbol(&mut store, "Foo");
        let first = store.scopes.alloc(Scope::new(Some(foo)));
        let second = store.scopes.alloc(Scope::new(Some(foo)));
        let mut index = TastySemanticIndex::new();
        index.insert_scope(foo, first).unwrap();

        let result = index.insert_scope(foo, second);

        assert_eq!(result, Err(UnpickleError::DuplicateScope { symbol: foo }));
        assert_eq!(index.scope_of(foo), Some(first));
    }

    // --- annotation tails ---

    #[test]
    fn an_address_with_no_recorded_annotation_tail_is_unknown() {
        let index = TastySemanticIndex::new();

        assert_eq!(index.annotation_tail_at(4), None);
        assert_eq!(index.annotated_definition_count(), 0);
        assert_eq!(index.annotation_tail_count(), 0);
    }

    #[test]
    fn a_definition_entered_with_no_annotations_is_recorded_as_empty_not_unknown() {
        let mut index = TastySemanticIndex::new();

        index.insert_annotation_tail(4, Vec::new());

        assert_eq!(index.annotation_tail_at(4), Some(&[][..]));
        assert_eq!(index.annotated_definition_count(), 0);
        assert_eq!(index.annotation_tail_count(), 0);
    }

    #[test]
    fn a_definitions_annotation_addresses_are_kept_in_wire_order() {
        let mut index = TastySemanticIndex::new();

        index.insert_annotation_tail(4, vec![9, 30, 21]);

        assert_eq!(index.annotation_tail_at(4), Some(&[9, 30, 21][..]));
        assert_eq!(index.annotated_definition_count(), 1);
        assert_eq!(index.annotation_tail_count(), 3);
    }

    #[test]
    fn distinct_addresses_keep_distinct_annotation_tails() {
        let mut index = TastySemanticIndex::new();

        index.insert_annotation_tail(4, vec![9]);
        index.insert_annotation_tail(46, Vec::new());
        index.insert_annotation_tail(90, vec![91, 92]);

        assert_eq!(index.annotation_tail_at(4), Some(&[9][..]));
        assert_eq!(index.annotation_tail_at(46), Some(&[][..]));
        assert_eq!(index.annotation_tail_at(90), Some(&[91, 92][..]));
        assert_eq!(index.annotated_definition_count(), 2);
        assert_eq!(index.annotation_tail_count(), 3);
    }
}
