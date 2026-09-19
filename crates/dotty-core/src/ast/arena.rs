//! Allocates and looks up [`Tree`] values for one phase.

use crate::ast::phase::AstPhase;
use crate::ast::tree::Tree;
use crate::ids::{TreeId, checked_index};

/// Owns every [`Tree`] for one phase (`Untyped` or `Typed`) of one
/// compilation session.
///
/// No `Box`/`Rc`/`Arc` per node: children are referenced by `TreeId<P>` into
/// this same arena.
#[derive(Debug)]
pub struct AstArena<P: AstPhase> {
    nodes: Vec<Tree<P>>,
}

impl<P: AstPhase> Default for AstArena<P> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

impl<P: AstPhase> AstArena<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, tree: Tree<P>) -> TreeId<P> {
        let id = TreeId::new(checked_index(self.nodes.len()));
        self.nodes.push(tree);
        id
    }

    /// Panics if `id` was not allocated by this arena — see
    /// `docs/dotty-core-design.md`, "Error handling policy."
    pub fn get(&self, id: TreeId<P>) -> &Tree<P> {
        &self.nodes[id.index() as usize]
    }

    pub fn get_mut(&mut self, id: TreeId<P>) -> &mut Tree<P> {
        &mut self.nodes[id.index() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::common::Ident;
    use crate::ast::phase::Untyped;
    use crate::ast::tree::TreeKind;
    use crate::ids::NameId;
    use crate::names::{Name, Namespace};

    fn ident_tree(raw: u32) -> Tree<Untyped> {
        Tree {
            kind: TreeKind::Ident(Ident {
                name: Name::new(NameId::new(raw), Namespace::Term),
                backquoted: false,
            }),
            position: None,
            ty: (),
        }
    }

    #[test]
    fn alloc_and_get_round_trip_a_tree() {
        let mut arena: AstArena<Untyped> = AstArena::new();
        let id = arena.alloc(ident_tree(1));

        assert_eq!(*arena.get(id), ident_tree(1));
    }

    #[test]
    fn an_error_node_can_be_allocated_in_an_untyped_arena() {
        let mut arena: AstArena<Untyped> = AstArena::new();
        let id = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(crate::ast::UntypedNode::Error(crate::ast::ErrorNode {
                kind: crate::ast::ErrorNodeKind::MissingType,
            })),
            position: None,
            ty: (),
        });

        assert!(matches!(
            arena.get(id).kind,
            TreeKind::PhaseSpecific(crate::ast::UntypedNode::Error(crate::ast::ErrorNode {
                kind: crate::ast::ErrorNodeKind::MissingType
            }))
        ));
    }

    #[test]
    fn distinct_allocations_get_distinct_ids() {
        let mut arena: AstArena<Untyped> = AstArena::new();
        let first = arena.alloc(ident_tree(1));
        let second = arena.alloc(ident_tree(2));

        assert_ne!(first, second);
    }

    #[test]
    fn get_mut_allows_updating_an_allocated_tree() {
        let mut arena: AstArena<Untyped> = AstArena::new();
        let id = arena.alloc(ident_tree(1));

        arena.get_mut(id).position = None;

        assert_eq!(arena.get(id).position, None);
    }
}
