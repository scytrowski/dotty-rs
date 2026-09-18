//! The address-keyed semantic index.
//!
//! TASTy AST addresses are the identity anchors of the format: a definition
//! is referred to by the address of its node, never by name. The index maps
//! each such address to the one `dotty-core` value that represents it, so a
//! later reference to the same address resolves to the same identity instead
//! of building a second one.

use std::collections::HashMap;

use dotty_core::ids::{ScopeId, SymbolId};

use crate::error::UnpickleError;

/// Maps TASTy definition addresses to the semantic entities entered for them.
///
/// The invariant is that one definition address owns exactly one
/// [`SymbolId`]. Addresses are absolute offsets in the AST section, as
/// reported by `dotty-tasty`'s address index.
///
/// Declaration scopes are keyed by the *owning symbol* rather than by an
/// address: a package can be spread over several `PACKAGE` nodes that share
/// one symbol and one scope, so no single node address identifies it. Maps
/// for shared types and typed trees are added by the milestones that
/// populate them.
#[derive(Debug, Default)]
pub struct TastySemanticIndex {
    symbols: HashMap<u32, SymbolId>,
    scopes: HashMap<SymbolId, ScopeId>,
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
}
