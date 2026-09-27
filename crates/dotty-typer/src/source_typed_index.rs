//! Mapping from source expression trees to their typed replacements.

use std::collections::HashMap;
use std::fmt;

use dotty_core::{SourceId, TreeId, Typed, Untyped};

/// A source tree already has a different typed tree recorded in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConflictingTypedTree {
    pub source: SourceId,
    pub untyped: TreeId<Untyped>,
    pub existing: TreeId<Typed>,
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
}

impl SourceTypedIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, source: SourceId, untyped: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.trees.get(&(source, untyped)).copied()
    }

    /// Records a mapping, allowing an idempotent re-insertion of the same tree.
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

    pub fn remove(&mut self, source: SourceId, untyped: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.trees.remove(&(source, untyped))
    }

    pub fn len(&self) -> usize {
        self.trees.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::ast::{AstArena, Ident, Tree, TreeKind};
    use dotty_core::{SemanticStore, Type};

    #[test]
    fn source_tree_lookup_is_scoped_by_source_id() {
        let mut untyped_arena = AstArena::<Untyped>::new();
        let mut store = SemanticStore::new();
        let name = store.names.intern("value");
        let source_tree = untyped_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let mut typed_arena = AstArena::<Typed>::new();
        let ty = store.types.alloc(Type::NoType);
        let typed_tree = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
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
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let mut typed_arena = AstArena::<Typed>::new();
        let ty = store.types.alloc(Type::NoType);
        let typed_tree = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
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
    fn conflicting_mapping_reports_existing_and_attempted_trees() {
        let mut untyped_arena = AstArena::<Untyped>::new();
        let mut store = SemanticStore::new();
        let name = store.names.intern("value");
        let source_tree = untyped_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        let mut typed_arena = AstArena::<Typed>::new();
        let first_ty = store.types.alloc(Type::NoType);
        let first = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: first_ty,
        });
        let second_ty = store.types.alloc(Type::NoType);
        let second = typed_arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
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
