//! Source-tree to semantic identity mappings produced by naming.

use std::collections::HashMap;

use dotty_core::{ScopeId, SourceId, SymbolId, TreeId, Untyped};

use crate::NamerError;

/// Opaque identifier for an immutable source declaration context.
///
/// IDs are meaningful only in the [`SourceSemanticIndex`] that allocated
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceContextId(u32);

impl SourceContextId {
    /// Returns the arena index backing this context identity.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// Lexical source environment captured for a declaration site.
///
/// `import` is the unresolved source import that transitions from `parent` to
/// this context. A context with no import is a base owner/scope environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceContext {
    /// Source lexical owner, independent of a declaration's semantic owner.
    pub owner: SymbolId,
    /// Lexical scope active at this source site.
    pub lexical_scope: ScopeId,
    /// Enclosing source environment inherited by this context.
    pub parent: Option<SourceContextId>,
    /// Unresolved import tree applied after the parent context, if any.
    pub import: Option<TreeId<Untyped>>,
}

/// Canonical and owner-specific derived identities associated with source
/// trees in compilation units.
///
/// Tree IDs are arena-relative, so lookups use both the source identity and
/// the tree ID. This index belongs to one naming result; persistent symbols
/// and scopes remain in [`dotty_core::SemanticStore`].
#[derive(Debug, Default)]
pub struct SourceSemanticIndex {
    symbols_by_tree: HashMap<(SourceId, TreeId<Untyped>), SymbolId>,
    derived_symbols_by_owner_and_tree: HashMap<(SymbolId, SourceId, TreeId<Untyped>), SymbolId>,
    scopes_by_owner: HashMap<SymbolId, ScopeId>,
    extension_prefix_clauses_by_method: HashMap<SymbolId, Vec<Vec<TreeId<Untyped>>>>,
    source_contexts: Vec<SourceContext>,
    declaration_contexts_by_symbol: HashMap<SymbolId, SourceContextId>,
}

impl SourceSemanticIndex {
    /// Creates an empty source-side semantic index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the canonical symbol represented by a tree in the given source
    /// unit. Derived symbols are available through [`Self::derived_symbol_at`]
    /// and never replace this mapping.
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

    /// Returns the original source prefix clauses for an extension method.
    ///
    /// The returned clauses preserve their source order and point to the
    /// original parameter trees. Ordinary methods return `None`.
    pub fn extension_prefix_clauses(&self, method: SymbolId) -> Option<&[Vec<TreeId<Untyped>>]> {
        self.extension_prefix_clauses_by_method
            .get(&method)
            .map(Vec::as_slice)
    }

    /// Returns an immutable source context owned by this index.
    ///
    /// Passing an ID from another index, or an out-of-range ID, is an
    /// internal arena invariant violation and panics, like other arena IDs.
    pub fn source_context(&self, id: SourceContextId) -> &SourceContext {
        self.source_contexts
            .get(id.index() as usize)
            .expect("SourceContextId must belong to this SourceSemanticIndex")
    }

    /// Returns the source declaration context recorded for `symbol`.
    pub fn declaration_context_of(&self, symbol: SymbolId) -> Option<SourceContextId> {
        self.declaration_contexts_by_symbol.get(&symbol).copied()
    }

    /// Records one immutable source context node and returns its index-local
    /// identity.
    pub(crate) fn alloc_source_context(&mut self, context: SourceContext) -> SourceContextId {
        let id = SourceContextId(
            u32::try_from(self.source_contexts.len())
                .unwrap_or_else(|_| panic!("source context arena exceeded u32::MAX entries")),
        );
        self.source_contexts.push(context);
        id
    }

    /// Associates a declaration symbol with its source lexical context.
    ///
    /// Repeating the same association is idempotent. Assigning a different
    /// context to the same symbol is an internal naming error and preserves
    /// the original association.
    pub(crate) fn record_declaration_context(
        &mut self,
        symbol: SymbolId,
        context: SourceContextId,
    ) -> Result<(), NamerError> {
        match self.declaration_contexts_by_symbol.get(&symbol) {
            Some(existing) if *existing == context => Ok(()),
            Some(existing) => Err(NamerError::DuplicateDeclarationContext {
                symbol,
                existing: *existing,
                attempted: context,
            }),
            None => {
                self.declaration_contexts_by_symbol.insert(symbol, context);
                Ok(())
            }
        }
    }

    /// Records the original extension prefix clauses for one method.
    ///
    /// Metadata is source structure, not a semantic parameter list. A method
    /// can only be registered once; duplicate registration leaves the first
    /// value intact.
    pub fn record_extension_prefix_clauses(
        &mut self,
        method: SymbolId,
        clauses: &[Vec<TreeId<Untyped>>],
    ) -> Result<(), NamerError> {
        if self
            .extension_prefix_clauses_by_method
            .contains_key(&method)
        {
            return Err(NamerError::DuplicateExtensionPrefixClauses { method });
        }
        self.extension_prefix_clauses_by_method
            .insert(method, clauses.to_vec());
        Ok(())
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
    fn source_contexts_are_stored_and_retrievable_by_opaque_id() {
        let mut store = SemanticStore::new();
        let owner = symbol(&mut store);
        let lexical_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();
        let context = SourceContext {
            owner,
            lexical_scope,
            parent: None,
            import: None,
        };

        let id = index.alloc_source_context(context);

        assert_eq!(id.index(), 0);
        assert_eq!(index.source_context(id), &context);
    }

    #[test]
    fn a_symbol_gets_one_declaration_context_and_repeated_same_context_is_idempotent() {
        let mut store = SemanticStore::new();
        let declaration = symbol(&mut store);
        let owner = symbol(&mut store);
        let lexical_scope = store.scopes.alloc(Scope::new(Some(owner)));
        let mut index = SourceSemanticIndex::new();
        let context = index.alloc_source_context(SourceContext {
            owner,
            lexical_scope,
            parent: None,
            import: None,
        });

        index
            .record_declaration_context(declaration, context)
            .unwrap();
        index
            .record_declaration_context(declaration, context)
            .unwrap();

        assert_eq!(index.declaration_context_of(declaration), Some(context));
    }

    #[test]
    fn assigning_a_different_declaration_context_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let declaration = symbol(&mut store);
        let first_owner = symbol(&mut store);
        let second_owner = symbol(&mut store);
        let first_scope = store.scopes.alloc(Scope::new(Some(first_owner)));
        let second_scope = store.scopes.alloc(Scope::new(Some(second_owner)));
        let mut index = SourceSemanticIndex::new();
        let first = index.alloc_source_context(SourceContext {
            owner: first_owner,
            lexical_scope: first_scope,
            parent: None,
            import: None,
        });
        let second = index.alloc_source_context(SourceContext {
            owner: second_owner,
            lexical_scope: second_scope,
            parent: Some(first),
            import: None,
        });
        index
            .record_declaration_context(declaration, first)
            .unwrap();

        assert_eq!(
            index.record_declaration_context(declaration, second),
            Err(NamerError::DuplicateDeclarationContext {
                symbol: declaration,
                existing: first,
                attempted: second,
            })
        );
        assert_eq!(index.declaration_context_of(declaration), Some(first));
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

    #[test]
    fn extension_prefix_clauses_preserve_the_original_clause_and_tree_order() {
        let mut store = SemanticStore::new();
        let method = symbol(&mut store);
        let mut arena = dotty_core::AstArena::<Untyped>::new();
        let first = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let second = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let clauses = vec![vec![first], vec![second, first]];
        let mut index = SourceSemanticIndex::new();

        index
            .record_extension_prefix_clauses(method, &clauses)
            .unwrap();

        assert_eq!(
            index.extension_prefix_clauses(method),
            Some(clauses.as_slice())
        );
        assert_eq!(index.extension_prefix_clauses(symbol(&mut store)), None);
    }

    #[test]
    fn duplicate_extension_prefix_clause_registration_is_rejected_without_replacement() {
        let mut store = SemanticStore::new();
        let method = symbol(&mut store);
        let mut arena = dotty_core::AstArena::<Untyped>::new();
        let first = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let second = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let first_clauses = vec![vec![first]];
        let second_clauses = vec![vec![second]];
        let mut index = SourceSemanticIndex::new();

        index
            .record_extension_prefix_clauses(method, &first_clauses)
            .unwrap();
        assert_eq!(
            index.record_extension_prefix_clauses(method, &second_clauses),
            Err(NamerError::DuplicateExtensionPrefixClauses { method })
        );
        assert_eq!(
            index.extension_prefix_clauses(method),
            Some(first_clauses.as_slice())
        );
    }
}
