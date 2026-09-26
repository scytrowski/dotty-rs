//! Typer-owned projections from source type trees to semantic types.

use std::collections::HashMap;

use dotty_core::{SourceId, TreeId, TypeId, Untyped};

/// Cache of semantic type-tree projections, kept separate from naming data.
#[derive(Debug, Default, Clone)]
pub struct SourceTypeIndex {
    types_by_tree: HashMap<(SourceId, TreeId<Untyped>), TypeId>,
}

impl SourceTypeIndex {
    /// Returns the semantic type already projected for this source tree.
    pub fn type_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TypeId> {
        self.types_by_tree.get(&(source, tree)).copied()
    }

    pub(crate) fn insert(
        &mut self,
        source: SourceId,
        tree: TreeId<Untyped>,
        ty: TypeId,
    ) -> Result<(), TypeId> {
        match self.types_by_tree.get(&(source, tree)).copied() {
            Some(existing) => Err(existing),
            None => {
                self.types_by_tree.insert((source, tree), ty);
                Ok(())
            }
        }
    }

    pub(crate) fn checkpoint(&self) -> Self {
        self.clone()
    }

    pub(crate) fn restore(&mut self, checkpoint: Self) {
        *self = checkpoint;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::AstArena;

    #[test]
    fn source_type_index_looks_up_by_source_and_tree() {
        let mut arena = AstArena::<Untyped>::new();
        let tree = arena.alloc(dotty_core::Tree {
            kind: dotty_core::TreeKind::TypeTree(dotty_core::ast::TypeTree),
            position: None,
            ty: (),
        });
        let source = SourceId::from_index(2);
        let mut store = dotty_core::SemanticStore::new();
        let ty = store.types.alloc(dotty_core::types::Type::NoPrefix);
        let mut index = SourceTypeIndex::default();

        index.insert(source, tree, ty).unwrap();

        assert_eq!(index.type_at(source, tree), Some(ty));
        assert_eq!(index.type_at(SourceId::from_index(3), tree), None);
    }

    #[test]
    fn restoring_a_cache_checkpoint_discards_only_later_entries() {
        let mut arena = AstArena::<Untyped>::new();
        let first = arena.alloc(dotty_core::Tree {
            kind: dotty_core::TreeKind::TypeTree(dotty_core::ast::TypeTree),
            position: None,
            ty: (),
        });
        let second = arena.alloc(dotty_core::Tree {
            kind: dotty_core::TreeKind::TypeTree(dotty_core::ast::TypeTree),
            position: None,
            ty: (),
        });
        let source = SourceId::from_index(2);
        let mut store = dotty_core::SemanticStore::new();
        let first_type = store.types.alloc(dotty_core::types::Type::NoPrefix);
        let second_type = store.types.alloc(dotty_core::types::Type::NoType);
        let mut index = SourceTypeIndex::default();
        index.insert(source, first, first_type).unwrap();
        let mark = index.checkpoint();
        index.insert(source, second, second_type).unwrap();

        index.restore(mark);

        assert_eq!(index.type_at(source, first), Some(first_type));
        assert_eq!(index.type_at(source, second), None);
    }
}
