//! Semantic annotations, shared by `Type::Annotated` and `Symbol::annotations`.
//!
//! See `docs/dotty-core-design.md`, `[MAJOR 3]`: real Dotty stores
//! annotations in two places — a list directly on the symbol denotation, and
//! a single annotation on an annotated type — both of which resolve through
//! one [`Annotation`]/[`AnnotationArena`] here.

use crate::ast::Typed;
use crate::ids::{AnnotationId, TreeId, TypeId};

/// One semantic annotation, e.g. `@deprecated` on a symbol or `@unchecked`
/// on a type.
///
/// `tree` is optional because an annotation may be known only by its class
/// `TypeId` before argument trees are modeled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub ty: TypeId,
    pub tree: Option<TreeId<Typed>>,
}

impl Annotation {
    pub const fn new(ty: TypeId, tree: Option<TreeId<Typed>>) -> Self {
        Self { ty, tree }
    }
}

/// Allocates and looks up [`Annotation`] values.
#[derive(Debug, Default)]
pub struct AnnotationArena {
    annotations: Vec<Annotation>,
}

impl AnnotationArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, annotation: Annotation) -> AnnotationId {
        let id = AnnotationId::new(self.annotations.len() as u32);
        self.annotations.push(annotation);
        id
    }

    /// Panics if `id` was not allocated by this arena — see
    /// `docs/dotty-core-design.md`, "Error handling policy."
    pub fn get(&self, id: AnnotationId) -> &Annotation {
        &self.annotations[id.index() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_and_resolves_an_annotation_without_a_tree() {
        let mut arena = AnnotationArena::new();
        let ty = TypeId::new(1);

        let id = arena.alloc(Annotation::new(ty, None));

        assert_eq!(arena.get(id).ty, ty);
        assert_eq!(arena.get(id).tree, None);
    }

    #[test]
    fn distinct_allocations_get_distinct_ids() {
        let mut arena = AnnotationArena::new();
        let first = arena.alloc(Annotation::new(TypeId::new(1), None));
        let second = arena.alloc(Annotation::new(TypeId::new(2), None));

        assert_ne!(first, second);
    }
}
