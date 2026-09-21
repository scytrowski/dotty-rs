//! Structural member lookup: what a name-designated reference selects.
//!
//! A `TypeRef` / `TermRef` with a [`Name`](crate::names::Name) target
//! (see [`crate::types::TypeRefTarget`]) has no declaration symbol: its member
//! lives in a `Refined` graph. This module reads that graph. It is the only
//! interpretation of a name target here, and it takes no part in building one:
//! a reference can be constructed while the `Recursive` it hangs from is
//! still being read, and looked up once the graph is complete.
//!
//! Supported graph: `Refined`, `Recursive`, `RecThis`, and the `Flexible` /
//! `Annotated` proxies, which have their underlying type's members. Anything
//! else has no structural members (`NotFound`): a class member is a declaration
//! and is found through symbols, not here.
//!
//! No substitution is done: a `RecThis` is looked through to the parent of its
//! binder, and the info found is returned as it is. For the canonical
//! `RecThis` of that same binder (the case a recursive refinement's own
//! references have) opening the recursive type is the identity, so the
//! returned info stays tied to the same binder.

use std::fmt;

use crate::ids::TypeId;
use crate::names::Name;
use crate::store::SemanticStore;
use crate::types::Type;

/// How many links (`Refined` parents, `Recursive`, `RecThis`, proxies) a
/// lookup follows before giving up: real graphs nest a handful deep, and a
/// cyclic or absurd one must end.
pub const MAX_STRUCTURAL_DEPTH: usize = 256;

/// The result of a structural member lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructuralMemberLookup {
    /// The nearest enclosing refinement of `name`; its info, as stored.
    Found { name: Name, info: TypeId },
    /// No refinement in the graph names `name`.
    NotFound,
}

/// The graph could not be read soundly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructuralLookupError {
    /// A type on the path is a binder slot that has not been filled yet.
    Unfilled { ty: TypeId },
    /// A `RecThis` names a binder that is not a `Recursive`.
    InvalidBinder { rec_this: TypeId },
    /// The path is longer than [`MAX_STRUCTURAL_DEPTH`].
    TooDeep,
}

impl fmt::Display for StructuralLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unfilled { .. } => write!(f, "a binder on the path is not complete yet"),
            Self::InvalidBinder { .. } => {
                write!(f, "a RecThis names a binder that is not recursive")
            }
            Self::TooDeep => write!(f, "the structural path is too deep"),
        }
    }
}

impl std::error::Error for StructuralLookupError {}

/// The refinement of `name` visible from `prefix`.
///
/// Nested refinements are read outermost first, as written: the outer
/// matching refinement wins, otherwise the walk continues into the parent.
/// Nothing is flattened or reordered, and the namespace is part of the
/// [`Name`], so a type `T` never matches a term `T`.
pub fn lookup_structural_member(
    store: &SemanticStore,
    prefix: TypeId,
    name: Name,
) -> Result<StructuralMemberLookup, StructuralLookupError> {
    let mut current = prefix;
    for _ in 0..=MAX_STRUCTURAL_DEPTH {
        if !store.types.is_filled(current) {
            return Err(StructuralLookupError::Unfilled { ty: current });
        }
        current = match store.types.get(current) {
            Type::Refined {
                parent,
                name: member,
                info,
            } => {
                if *member == name {
                    return Ok(StructuralMemberLookup::Found { name, info: *info });
                }
                *parent
            }
            Type::Recursive { parent } => *parent,
            Type::RecThis { binder } => {
                if !store.types.is_filled(*binder) {
                    return Err(StructuralLookupError::Unfilled { ty: *binder });
                }
                match store.types.get(*binder) {
                    Type::Recursive { parent } => *parent,
                    _ => {
                        return Err(StructuralLookupError::InvalidBinder { rec_this: current });
                    }
                }
            }
            Type::Flexible { underlying } | Type::Annotated { underlying, .. } => *underlying,
            _ => return Ok(StructuralMemberLookup::NotFound),
        };
    }
    Err(StructuralLookupError::TooDeep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::Namespace;
    use crate::types::Annotation;

    struct World {
        store: SemanticStore,
        leaf: TypeId,
    }

    impl World {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let leaf = store.types.alloc(Type::NoPrefix);
            Self { store, leaf }
        }

        fn name(&mut self, text: &str, namespace: Namespace) -> Name {
            Name::new(self.store.names.intern(text), namespace)
        }

        fn refined(&mut self, parent: TypeId, name: Name, info: TypeId) -> TypeId {
            self.store.types.alloc(Type::Refined { parent, name, info })
        }
    }

    #[test]
    fn the_outer_matching_refinement_wins_and_order_is_kept() {
        let mut w = World::new();
        let t = w.name("T", Namespace::Type);
        let (inner_info, outer_info) = (
            w.store.types.alloc(Type::NoType),
            w.store.types.alloc(Type::NoType),
        );
        let leaf = w.leaf;
        let inner = w.refined(leaf, t, inner_info);
        let outer = w.refined(inner, t, outer_info);

        assert_eq!(
            lookup_structural_member(&w.store, outer, t),
            Ok(StructuralMemberLookup::Found {
                name: t,
                info: outer_info
            })
        );
        // The inner one is what a lookup from the inner refinement sees.
        assert_eq!(
            lookup_structural_member(&w.store, inner, t),
            Ok(StructuralMemberLookup::Found {
                name: t,
                info: inner_info
            })
        );
    }

    #[test]
    fn a_parent_refinement_is_searched_when_the_outer_one_names_another_member() {
        let mut w = World::new();
        let (t1, t2) = (w.name("T1", Namespace::Type), w.name("T2", Namespace::Type));
        let info1 = w.store.types.alloc(Type::NoType);
        let leaf = w.leaf;
        let inner = w.refined(leaf, t1, info1);
        let outer = w.refined(inner, t2, leaf);

        assert_eq!(
            lookup_structural_member(&w.store, outer, t1),
            Ok(StructuralMemberLookup::Found {
                name: t1,
                info: info1
            })
        );
    }

    #[test]
    fn a_type_and_a_term_of_the_same_text_are_different_members() {
        let mut w = World::new();
        let ty = w.name("m", Namespace::Type);
        let term = w.name("m", Namespace::Term);
        let info = w.store.types.alloc(Type::NoType);
        let leaf = w.leaf;
        let refined = w.refined(leaf, ty, info);

        assert_eq!(
            lookup_structural_member(&w.store, refined, term),
            Ok(StructuralMemberLookup::NotFound)
        );
    }

    #[test]
    fn a_type_with_no_refinement_has_no_structural_members() {
        let w = World::new();
        let mut w = w;
        let t = w.name("T", Namespace::Type);
        let leaf = w.leaf;

        assert_eq!(
            lookup_structural_member(&w.store, leaf, t),
            Ok(StructuralMemberLookup::NotFound)
        );
    }

    /// `Recursive { Refined T }` with a `RecThis` of it, and the info naming
    /// that same `RecThis`.
    fn recursive_world() -> (World, TypeId, TypeId, TypeId, Name) {
        let mut w = World::new();
        let t = w.name("T", Namespace::Type);
        let binder = w.store.types.reserve();
        let rec_this = w.store.types.alloc(Type::RecThis {
            binder: binder.id(),
        });
        let leaf = w.leaf;
        let info = w
            .store
            .types
            .alloc(Type::AliasingBounds { alias: rec_this });
        let refined = w.refined(leaf, t, info);
        w.store
            .types
            .fill(binder, Type::Recursive { parent: refined });
        (w, binder.id(), rec_this, info, t)
    }

    #[test]
    fn recursive_and_rec_this_delegate_to_the_parent_and_the_info_stays_tied() {
        let (w, recursive, rec_this, info, t) = recursive_world();
        let found = Ok(StructuralMemberLookup::Found { name: t, info });

        assert_eq!(lookup_structural_member(&w.store, recursive, t), found);
        assert_eq!(lookup_structural_member(&w.store, rec_this, t), found);
        // The info names the very `RecThis` of the same binder.
        let Type::AliasingBounds { alias } = w.store.types.get(info) else {
            panic!("alias expected");
        };
        assert_eq!(*alias, rec_this);
        assert!(matches!(
            w.store.types.get(*alias),
            Type::RecThis { binder } if *binder == recursive
        ));
    }

    #[test]
    fn flexible_and_annotated_proxies_are_looked_through() {
        let (mut w, recursive, _, info, t) = recursive_world();
        let flexible = w.store.types.alloc(Type::Flexible {
            underlying: recursive,
        });
        let annotation = w.store.annotations.alloc(Annotation::new(w.leaf, None));
        let annotated = w.store.types.alloc(Type::Annotated {
            underlying: flexible,
            annotation,
        });

        assert_eq!(
            lookup_structural_member(&w.store, annotated, t),
            Ok(StructuralMemberLookup::Found { name: t, info })
        );
    }

    #[test]
    fn a_binder_that_is_not_filled_is_a_typed_error_not_a_panic() {
        let mut w = World::new();
        let t = w.name("T", Namespace::Type);
        let binder = w.store.types.reserve();
        let rec_this = w.store.types.alloc(Type::RecThis {
            binder: binder.id(),
        });

        assert_eq!(
            lookup_structural_member(&w.store, rec_this, t),
            Err(StructuralLookupError::Unfilled { ty: binder.id() })
        );
        assert_eq!(
            lookup_structural_member(&w.store, binder.id(), t),
            Err(StructuralLookupError::Unfilled { ty: binder.id() })
        );
    }

    #[test]
    fn a_rec_this_of_something_that_is_not_recursive_is_invalid() {
        let mut w = World::new();
        let t = w.name("T", Namespace::Type);
        let leaf = w.leaf;
        let rec_this = w.store.types.alloc(Type::RecThis { binder: leaf });

        assert_eq!(
            lookup_structural_member(&w.store, rec_this, t),
            Err(StructuralLookupError::InvalidBinder { rec_this })
        );
    }

    #[test]
    fn a_cyclic_or_very_deep_path_ends_with_an_error() {
        let mut w = World::new();
        let t = w.name("T", Namespace::Type);
        // `Recursive` whose parent is its own `RecThis`: a cycle.
        let binder = w.store.types.reserve();
        let rec_this = w.store.types.alloc(Type::RecThis {
            binder: binder.id(),
        });
        w.store
            .types
            .fill(binder, Type::Recursive { parent: rec_this });

        assert_eq!(
            lookup_structural_member(&w.store, rec_this, t),
            Err(StructuralLookupError::TooDeep)
        );

        let mut deep = w.leaf;
        for _ in 0..MAX_STRUCTURAL_DEPTH + 2 {
            deep = w.store.types.alloc(Type::Flexible { underlying: deep });
        }
        assert_eq!(
            lookup_structural_member(&w.store, deep, t),
            Err(StructuralLookupError::TooDeep)
        );
    }
}
