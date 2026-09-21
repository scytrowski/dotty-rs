//! Semantic annotations, shared by `Type::Annotated` and `Symbol::annotations`.
//!
//! See `docs/dotty-core-design.md`, `[MAJOR 3]`: real Dotty stores
//! annotations in two places — a list directly on the symbol denotation, and
//! a single annotation on an annotated type — both of which resolve through
//! one [`Annotation`]/[`AnnotationArena`] here.

use crate::ast::Typed;
use crate::ids::{AnnotationId, TreeId, TypeId, checked_index};
use crate::names::TermName;
use crate::types::constant::Constant;

/// One semantic annotation, e.g. `@deprecated` on a symbol or `@unchecked`
/// on a type.
///
/// `ty` is the annotation's type, which for a parameterised annotation is its
/// whole applied type. `arguments` are its term arguments, if the producer
/// reconstructed them. `tree` is an optional typed tree for later fidelity;
/// `None` means "no reconstructed typed tree is attached", not "the
/// annotation had no payload": [`AnnotationArguments`] carries that
/// distinction.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub ty: TypeId,
    pub arguments: AnnotationArguments,
    pub tree: Option<TreeId<Typed>>,
}

impl Annotation {
    /// An annotation whose term arguments the producer did not reconstruct
    /// ([`AnnotationArguments::Unavailable`]). This is what an annotation known
    /// only by its class `TypeId` is: it is never read as "no arguments".
    pub const fn new(ty: TypeId, tree: Option<TreeId<Typed>>) -> Self {
        Self {
            ty,
            arguments: AnnotationArguments::Unavailable,
            tree,
        }
    }

    /// A compact annotation: its type is the whole annotation, so it is known
    /// to have no term arguments and needs no tree. Type arguments stay in `ty`.
    pub const fn compact(ty: TypeId) -> Self {
        Self {
            ty,
            arguments: AnnotationArguments::Known(Vec::new()),
            tree: None,
        }
    }

    /// An annotation with exactly the term `arguments` given, in order, and
    /// no typed tree.
    pub fn with_arguments(ty: TypeId, arguments: Vec<AnnotationArgument>) -> Self {
        Self {
            ty,
            arguments: AnnotationArguments::Known(arguments),
            tree: None,
        }
    }
}

/// The term arguments of an [`Annotation`].
///
/// `Unavailable` and `Known(vec![])` are different facts: the first says the
/// producer did not look at the arguments, the second that there are none.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationArguments {
    /// Not reconstructed by the producer.
    Unavailable,
    /// Every term argument, in the order written; type arguments are part of
    /// the annotation's type, not of this list.
    Known(Vec<AnnotationArgument>),
}

/// One term argument of an annotation.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationArgument {
    /// The parameter name of a named argument (`name = value`), `None` for a
    /// positional one.
    pub name: Option<TermName>,
    pub value: AnnotationValue,
}

/// The value of an annotation argument: a closed, format-agnostic set that
/// grows only when a real form justifies a variant.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationValue {
    /// A literal, including a class literal (`Constant::Class`).
    Constant(Constant),
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
        let id = AnnotationId::new(checked_index(self.annotations.len()));
        self.annotations.push(annotation);
        id
    }

    /// The number of annotations allocated so far.
    pub(crate) fn len(&self) -> usize {
        self.annotations.len()
    }

    /// Drops every annotation allocated after the arena held `len`.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.annotations.truncate(len);
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
    fn unavailable_arguments_are_not_known_to_be_empty() {
        let ty = TypeId::new(1);

        assert_eq!(
            Annotation::new(ty, None).arguments,
            AnnotationArguments::Unavailable
        );
        assert_eq!(
            Annotation::compact(ty).arguments,
            AnnotationArguments::Known(Vec::new())
        );
        assert_ne!(Annotation::new(ty, None), Annotation::compact(ty));
        assert_ne!(
            Annotation::new(ty, None),
            Annotation::with_arguments(ty, Vec::new())
        );
    }

    #[test]
    fn arguments_keep_their_order_and_names() {
        let mut arena = AnnotationArena::new();
        let arguments = vec![
            AnnotationArgument {
                name: None,
                value: AnnotationValue::Constant(Constant::Int(1)),
            },
            AnnotationArgument {
                name: Some(TermName::new(crate::ids::NameId::new(7))),
                value: AnnotationValue::Constant(Constant::Int(2)),
            },
            AnnotationArgument {
                name: Some(TermName::new(crate::ids::NameId::new(7))),
                value: AnnotationValue::Constant(Constant::Int(3)),
            },
        ];

        let id = arena.alloc(Annotation::with_arguments(
            TypeId::new(1),
            arguments.clone(),
        ));

        // Duplicate names are kept as written, not merged.
        assert_eq!(
            arena.get(id).arguments,
            AnnotationArguments::Known(arguments)
        );
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
