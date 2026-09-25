//! Source-tree to semantic identity mappings produced by naming.

use std::collections::HashMap;

use dotty_core::{ScopeId, SourceId, SymbolId, TreeId, Untyped};

use crate::NamerError;

/// Semantic identities associated with trees in source compilation units.
///
/// Tree IDs are arena-relative, so lookups use both the source identity and
/// the tree ID. This index belongs to one naming result; persistent symbols
/// and scopes remain in [`dotty_core::SemanticStore`].
#[derive(Debug, Default)]
pub struct SourceSemanticIndex {
    symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    derived_symbols_by_owner_and_tree: HashMap<(SymbolId, SourceId, TreeId<Untyped>), SymbolId>,
    scopes_by_owner: HashMap<SymbolId, ScopeId>,
}

impl SourceSemanticIndex {
    /// Creates an empty source-side semantic index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the symbol assigned to a tree in the given source unit.
    pub fn symbol_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<SymbolId> {
        self.symbols_by_tree.get(&(source, tree)).copied()
    }

    /// Returns the derived symbol assigned to `tree` for the given `owner`.
    ///
    /// Derived identities are kept separate from [`Self::symbol_at`], which
    /// continues to return only the canonical identity represented by a
    /// source tree.
    pub fn derived_symbol_at(
        &self,
        owner: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
    ) -> Option<SymbolId> {
        self.derived_symbols_by_owner_and_tree
            .get(&(owner, source, tree))
            .copied()
    }

    /// Associates a derived declaration identity with its semantic owner.
    ///
    /// The same source tree may have derived identities for different owners,
    /// but a given `(owner, source, tree)` key can only be recorded once.
    /// Duplicate insertion is an internal naming error and leaves the
    /// existing mapping intact.
    pub fn record_derived_symbol(
        &mut self,
        owner: SymbolId,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    ) -> Result<(), NamerError> {
        let key = (owner, source, tree);
        if self.derived_symbols_by_owner_and_tree.contains_key(&key) {
            return Err(NamerError::DuplicateDerivedSourceTreeSymbol {
                owner,
                source,
                tree_index: tree.index(),
            });
        }
        self.derived_symbols_by_owner_and_tree.insert(key, symbol);
        Ok(())
    }

    /// Returns the declaration scope owned by `symbol`, if one was indexed.
    pub fn scope_of(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.scopes_by_owner.get(&symbol).copied()
    }

    /// Associates a definition tree with its symbol.
    ///
    /// A source tree can own at most one symbol. Duplicate insertion is an
    /// internal naming error and leaves the existing mapping intact.
    pub fn record_symbol(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    ) -> Result<(), NamerError> {
        let key = (source, tree);
        if self.symbols_by_tree.contains_key(&key) {
            return Err(NamerError::DuplicateSourceTreeSymbol {
                source,
                tree_index: tree.index(),
            });
        }
        self.symbols_by_tree.insert(key, symbol);
        Ok(())
    }

    /// Associates a declaration scope with its semantic owner.
    ///
    /// Duplicate scope registration is an internal naming error and leaves
    /// the existing mapping intact.
    pub fn record_scope(&mut self, symbol: SymbolId, scope: ScopeId) -> Result<(), NamerError> {
        match self.scopes_by_owner.get(&symbol) {
            Some(existing) if *existing == scope => Ok(()),
            Some(_) => Err(NamerError::DuplicateDeclarationScope { symbol }),
            None => {
                self.scopes_by_owner.insert(symbol, scope);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::ast::{ErrorNode, ErrorNodeKind, UntypedNode};
    use dotty_core::{
        Scope, SemanticStore, SourceId, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks,
        SymbolOrigin, Tree, TreeKind, Untyped, Visibility,
    };

    use super::*;

    fn symbol(store: &mut SemanticStore) -> SymbolId {
        let name_id = store.names.intern("test");
        let name = *dotty_core::TermName::new(name_id).as_name();
        store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Value,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn tree_id() -> TreeId<Untyped> {
        let mut arena = dotty_core::AstArena::<Untyped>::new();
        arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        })
    }

    #[test]
    fn symbol_identity_distinguishes_source_units_with_the_same_tree_id() {
        let mut store = SemanticStore::new();
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_symbol(SourceId::from_index(1), tree, first)
            .unwrap();
        index
            .record_symbol(SourceId::from_index(2), tree, second)
            .unwrap();

        assert_eq!(index.symbol_at(SourceId::from_index(1), tree), Some(first));
        assert_eq!(index.symbol_at(SourceId::from_index(2), tree), Some(second));
    }

    #[test]
    fn duplicate_source_tree_symbol_insertion_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let source = SourceId::from_index(1);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_symbol(source, tree, first).unwrap();
        assert!(matches!(
            index.record_symbol(source, tree, second),
            Err(NamerError::DuplicateSourceTreeSymbol {
                source: duplicate_source,
                tree_index: 0
            }) if duplicate_source == source
        ));
        assert_eq!(index.symbol_at(source, tree), Some(first));
    }

    #[test]
    fn duplicate_derived_source_tree_symbol_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let source = SourceId::from_index(1);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, first)
            .unwrap();
        assert!(matches!(
            index.record_derived_symbol(owner, source, tree, second),
            Err(NamerError::DuplicateDerivedSourceTreeSymbol {
                owner: duplicate_owner,
                source: duplicate_source,
                tree_index: 0
            }) if duplicate_owner == owner && duplicate_source == source
        ));
        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(first));
    }

    #[test]
    fn canonical_and_derived_source_identities_coexist() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let canonical = symbol(&mut store);
        let derived = symbol(&mut store);
        let source = SourceId::from_index(3);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_symbol(source, tree, canonical).unwrap();
        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.symbol_at(source, tree), Some(canonical));
        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(derived));
    }

    #[test]
    fn derived_identity_is_not_returned_by_canonical_lookup() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let derived = symbol(&mut store);
        let source = SourceId::from_index(4);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.symbol_at(source, tree), None);
        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(derived));
    }

    #[test]
    fn one_source_tree_can_have_derived_identities_for_multiple_owners() {
        let mut store = SemanticStore::new();
        let first_owner = symbol(&mut store);
        let second_owner = symbol(&mut store);
        let first_derived = symbol(&mut store);
        let second_derived = symbol(&mut store);
        let source = SourceId::from_index(5);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(first_owner, source, tree, first_derived)
            .unwrap();
        index
            .record_derived_symbol(second_owner, source, tree, second_derived)
            .unwrap();

        assert_eq!(
            index.derived_symbol_at(first_owner, source, tree),
            Some(first_derived)
        );
        assert_eq!(
            index.derived_symbol_at(second_owner, source, tree),
            Some(second_derived)
        );
    }

    #[test]
    fn derived_lookup_requires_the_exact_owner() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let other_owner = symbol(&mut store);
        let derived = symbol(&mut store);
        let source = SourceId::from_index(6);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.derived_symbol_at(owner, source, tree), Some(derived));
        assert_eq!(index.derived_symbol_at(other_owner, source, tree), None);
    }

    #[test]
    fn derived_identity_distinguishes_sources_with_the_same_tree_id() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let first = symbol(&mut store);
        let second = symbol(&mut store);
        let tree = tree_id();
        let first_source = SourceId::from_index(7);
        let second_source = SourceId::from_index(8);
        let mut index = SourceSemanticIndex::new();

        index
            .record_derived_symbol(owner, first_source, tree, first)
            .unwrap();
        index
            .record_derived_symbol(owner, second_source, tree, second)
            .unwrap();

        assert_eq!(
            index.derived_symbol_at(owner, first_source, tree),
            Some(first)
        );
        assert_eq!(
            index.derived_symbol_at(owner, second_source, tree),
            Some(second)
        );
    }

    #[test]
    fn registering_a_derived_identity_does_not_change_scope_lookup() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let derived = symbol(&mut store);
        let scope = store.scopes.alloc(Scope::new(Some(owner)));
        let source = SourceId::from_index(9);
        let tree = tree_id();
        let mut index = SourceSemanticIndex::new();

        index.record_scope(owner, scope).unwrap();
        index
            .record_derived_symbol(owner, source, tree, derived)
            .unwrap();

        assert_eq!(index.scope_of(owner), Some(scope));
        assert_eq!(index.scope_of(derived), None);
    }

    #[test]
    fn registering_the_same_scope_for_a_symbol_is_idempotent() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();

        index.record_scope(owner, scope).unwrap();
        index.record_scope(owner, scope).unwrap();

        assert_eq!(index.scope_of(owner), Some(scope));
    }

    #[test]
    fn duplicate_scope_registration_for_a_symbol_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let first_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let second_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();

        index.record_scope(owner, first_scope).unwrap();
        assert!(matches!(
            index.record_scope(owner, second_scope),
            Err(NamerError::DuplicateDeclarationScope { symbol }) if symbol == owner
        ));
        assert_eq!(index.scope_of(owner), Some(first_scope));
    }
}
