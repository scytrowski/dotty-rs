//! Mapping from source trees to their typed replacements.

use std::collections::HashMap;
use std::fmt;

use crate::ast::{AstArena, Tree};
use crate::{SourceId, SymbolId, TreeId, Typed, Untyped};

/// A source tree already has a different typed tree recorded in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConflictingTypedTree {
    /// Source unit whose mapping conflicted.
    pub source: SourceId,
    /// Untyped source tree with duplicate mappings.
    pub untyped: TreeId<Untyped>,
    /// Typed tree already stored.
    pub existing: TreeId<Typed>,
    /// Different typed tree that was attempted.
    pub attempted: TreeId<Typed>,
}

impl fmt::Display for ConflictingTypedTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ConflictingTypedTree {}

/// Stable source-to-typed identity for forms represented by one typed node.
#[derive(Clone, Debug, Default)]
pub struct SourceTypedIndex {
    trees: HashMap<(SourceId, TreeId<Untyped>), TreeId<Typed>>,
    named_calls: HashMap<TreeId<Typed>, NamedCallBinding>,
    synthetic_definitions: HashMap<TreeId<Typed>, SymbolId>,
}

/// Correspondence retained when a checked ordinary call contains argument labels.
#[derive(Clone, Debug)]
pub struct NamedCallBinding {
    /// Original typed callee before capturing the receiver in a temporary.
    pub function: TreeId<Typed>,
    /// Formal ordinal for each written operand, including positional operands.
    pub written_to_formal: Box<[usize]>,
}

impl SourceTypedIndex {
    /// Creates an empty source-to-typed mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up the typed replacement for one source tree.
    pub fn get(&self, source: SourceId, untyped: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.trees.get(&(source, untyped)).copied()
    }

    /// Records a source-to-typed mapping, allowing insertion of the same pair twice.
    pub fn insert(
        &mut self,
        source: SourceId,
        untyped: TreeId<Untyped>,
        typed: TreeId<Typed>,
    ) -> Result<(), ConflictingTypedTree> {
        if let Some(existing) = self.get(source, untyped) {
            if existing != typed {
                return Err(ConflictingTypedTree {
                    source,
                    untyped,
                    existing,
                    attempted: typed,
                });
            }
            return Ok(());
        }
        self.trees.insert((source, untyped), typed);
        Ok(())
    }

    /// Adds mappings into the same session arena. On conflict the index is
    /// unchanged and the first conflict in source and tree index order is returned.
    pub fn merge(&mut self, other: Self) -> Result<(), ConflictingTypedTree> {
        if self.is_empty() && self.named_calls.is_empty() && self.synthetic_definitions.is_empty() {
            *self = other;
            return Ok(());
        }
        let conflict = other
            .trees
            .iter()
            .filter_map(|(&(source, untyped), &attempted)| {
                let existing = self.get(source, untyped)?;
                (existing != attempted).then_some(ConflictingTypedTree {
                    source,
                    untyped,
                    existing,
                    attempted,
                })
            })
            .min_by_key(|error| (error.source.index(), error.untyped.index()));
        if let Some(error) = conflict {
            return Err(error);
        }
        self.trees.extend(other.trees);
        self.named_calls.extend(other.named_calls);
        self.synthetic_definitions
            .extend(other.synthetic_definitions);
        Ok(())
    }

    /// Removes the mapping for a source tree, if present.
    pub fn remove(&mut self, source: SourceId, untyped: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.trees.remove(&(source, untyped))
    }

    /// Records the exact typed root and pre-lowering function of an ordinary named call.
    /// All IDs must belong to this index's typed arena. The caller supplies a complete
    /// permutation of the selected clause in written order after successful typing.
    pub fn record_named_call(
        &mut self,
        root: TreeId<Typed>,
        function: TreeId<Typed>,
        written_to_formal: Box<[usize]>,
    ) {
        self.named_calls.insert(
            root,
            NamedCallBinding {
                function,
                written_to_formal,
            },
        );
    }

    /// Returns the argument binding of this exact typed call or its evaluation-order block.
    pub fn named_call(&self, root: TreeId<Typed>) -> Option<&NamedCallBinding> {
        self.named_calls.get(&root)
    }

    /// Allocates a compiler-generated definition and binds it to its symbol.
    /// The typed arena and index must belong to the same compiler session.
    pub fn alloc_synthetic_definition(
        &mut self,
        arena: &mut AstArena<Typed>,
        symbol: SymbolId,
        definition: Tree<Typed>,
    ) -> TreeId<Typed> {
        let tree = arena.alloc(definition);
        self.synthetic_definitions.insert(tree, symbol);
        tree
    }

    /// Returns the canonical symbol of a source-free typed definition.
    pub fn synthetic_definition_symbol(&self, tree: TreeId<Typed>) -> Option<SymbolId> {
        self.synthetic_definitions.get(&tree).copied()
    }

    /// Returns the number of recorded source trees.
    pub fn len(&self) -> usize {
        self.trees.len()
    }

    /// Returns whether the index contains no mappings.
    pub fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AstArena, Ident, Tree, TreeKind};
    use crate::{SemanticStore, Type};

    #[test]
    fn synthetic_definitions_with_the_same_name_bind_distinct_symbols() {
        let mut store = SemanticStore::new();
        let definitions = crate::Definitions::bootstrap(&mut store);
        let name = crate::TermName::new(store.names.intern("evidence"));
        let mut arena = AstArena::new();
        let tpt = crate::ast::TypedAstBuilder::new(&mut arena, &store.types)
            .type_tree(definitions.int, None);
        let definition = Tree {
            kind: TreeKind::ValDef(crate::ast::ValDef {
                name,
                tpt,
                rhs: None,
                metadata: (),
            }),
            ty: definitions.int,
            position: None,
        };
        let mut index = SourceTypedIndex::new();
        let first = SymbolId::new(1);
        let second = SymbolId::new(2);
        let first_tree = index.alloc_synthetic_definition(&mut arena, first, definition.clone());
        let second_tree = index.alloc_synthetic_definition(&mut arena, second, definition);
        assert_eq!(index.synthetic_definition_symbol(first_tree), Some(first));
        assert_eq!(index.synthetic_definition_symbol(second_tree), Some(second));
        assert_eq!(index.synthetic_definition_symbol(tpt), None);
    }

    #[test]
    fn source_tree_lookup_is_scoped_by_source_id() {
        let mut untyped_arena = AstArena::<Untyped>::new();
        let mut store = SemanticStore::new();
        let name = store.names.intern("value");
        let source_tree = untyped_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let mut typed_arena = AstArena::<Typed>::new();
        let ty = store.types.alloc(Type::NoType);
        let typed_tree = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty,
        });
        let mut index = SourceTypedIndex::new();
        let source_a = SourceId::from_index(1);
        let source_b = SourceId::from_index(2);

        index.insert(source_a, source_tree, typed_tree).unwrap();

        assert_eq!(index.get(source_a, source_tree), Some(typed_tree));
        assert_eq!(index.get(source_b, source_tree), None);
    }

    #[test]
    fn reinserting_the_same_mapping_is_idempotent() {
        let mut untyped_arena = AstArena::<Untyped>::new();
        let mut store = SemanticStore::new();
        let name = store.names.intern("value");
        let source_tree = untyped_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let mut typed_arena = AstArena::<Typed>::new();
        let ty = store.types.alloc(Type::NoType);
        let typed_tree = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty,
        });
        let mut index = SourceTypedIndex::new();
        let source = SourceId::from_index(1);

        index.insert(source, source_tree, typed_tree).unwrap();
        index.insert(source, source_tree, typed_tree).unwrap();

        assert_eq!(index.len(), 1);
    }

    #[test]
    fn merging_indexes_preserves_source_identity_and_rejects_conflicts_atomically() {
        let a = SourceId::from_index(0);
        let b = SourceId::from_index(1);
        let tree = TreeId::new(0);
        let first = TreeId::new(1);
        let second = TreeId::new(2);
        let mut index = SourceTypedIndex::new();
        index.insert(a, tree, first).unwrap();
        let mut batch = SourceTypedIndex::new();
        batch.insert(a, tree, first).unwrap();
        batch.insert(b, tree, second).unwrap();
        index.merge(batch).unwrap();
        assert_eq!(index.get(a, tree), Some(first));
        assert_eq!(index.get(b, tree), Some(second));

        let mut conflicting = SourceTypedIndex::new();
        conflicting.insert(b, tree, first).unwrap();
        conflicting.insert(a, TreeId::new(7), second).unwrap();
        conflicting.insert(a, tree, second).unwrap();
        assert_eq!(
            index.merge(conflicting),
            Err(ConflictingTypedTree {
                source: a,
                untyped: tree,
                existing: first,
                attempted: second
            })
        );
        assert_eq!(index.len(), 2);
        assert_eq!(index.get(a, tree), Some(first));
        assert_eq!(index.get(b, tree), Some(second));
    }

    #[test]
    fn conflicting_mapping_reports_existing_and_attempted_trees() {
        let mut untyped_arena = AstArena::<Untyped>::new();
        let mut store = SemanticStore::new();
        let name = store.names.intern("value");
        let source_tree = untyped_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let mut typed_arena = AstArena::<Typed>::new();
        let first_ty = store.types.alloc(Type::NoType);
        let first = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: first_ty,
        });
        let second_ty = store.types.alloc(Type::NoType);
        let second = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: crate::Name::new(name, crate::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: second_ty,
        });
        let mut index = SourceTypedIndex::new();
        let source = SourceId::from_index(1);
        index.insert(source, source_tree, first).unwrap();

        let error = index.insert(source, source_tree, second).unwrap_err();

        assert_eq!(error.existing, first);
        assert_eq!(error.attempted, second);
        assert_eq!(index.get(source, source_tree), Some(first));
    }
}
