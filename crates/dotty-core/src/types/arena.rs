//! Allocates and looks up [`Type`] values.

use crate::ids::TypeId;
use crate::types::binder::ReservedTypeId;
use crate::types::ty::Type;

/// Owns every [`Type`] value for one compilation session.
///
/// No interning in the foundation PR: two structurally equal `Poly`/
/// `TypeLambda` values are only truly interchangeable if their binders are
/// alpha-equivalent, which plain structural equality does not establish —
/// see `docs/dotty-core-design.md` §8.
#[derive(Debug, Default)]
pub struct TypeArena {
    types: Vec<Type>,
    /// Parallel to `types`: false for a slot created by `reserve` that has
    /// not yet been `fill`-ed. Reading such a slot is a compiler bug (a
    /// reservation that outlived the construction sequence that made it),
    /// not recoverable input — see §12's error-handling policy.
    filled: Vec<bool>,
}

impl TypeArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, ty: Type) -> TypeId {
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(ty);
        self.filled.push(true);
        id
    }

    /// Panics if `id` was not allocated by this arena, or was `reserve`-d but
    /// never `fill`-ed.
    pub fn get(&self, id: TypeId) -> &Type {
        let index = id.index() as usize;
        assert!(
            self.filled[index],
            "TypeId {index} was reserved but never filled"
        );
        &self.types[index]
    }

    /// Panics under the same conditions as [`TypeArena::get`].
    pub fn get_mut(&mut self, id: TypeId) -> &mut Type {
        let index = id.index() as usize;
        assert!(
            self.filled[index],
            "TypeId {index} was reserved but never filled"
        );
        &mut self.types[index]
    }

    /// Reserves a slot, backed by a `Type::NoType` placeholder, and returns
    /// its `TypeId` immediately so it can be used as a binder identity before
    /// the real value is known.
    pub fn reserve(&mut self) -> ReservedTypeId {
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(Type::NoType);
        self.filled.push(false);
        ReservedTypeId::from_type_id(id)
    }

    /// Overwrites a reserved slot with its real value.
    pub fn fill(&mut self, id: ReservedTypeId, ty: Type) -> TypeId {
        let index = id.id().index() as usize;
        self.types[index] = ty;
        self.filled[index] = true;
        id.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{NameId, SymbolId};
    use crate::names::{TermName, TypeName};
    use crate::types::method::{
        MethodKind, MethodParam, MethodType, PolyType, TypeParam, Variance,
    };

    #[test]
    fn alloc_and_get_round_trip_a_type() {
        let mut arena = TypeArena::new();
        let id = arena.alloc(Type::ThisType {
            class: SymbolId::new(1),
        });

        assert_eq!(
            *arena.get(id),
            Type::ThisType {
                class: SymbolId::new(1)
            }
        );
    }

    #[test]
    fn distinct_allocations_get_distinct_ids() {
        let mut arena = TypeArena::new();
        let first = arena.alloc(Type::NoType);
        let second = arena.alloc(Type::NoPrefix);

        assert_ne!(first, second);
    }

    #[test]
    #[should_panic(expected = "reserved but never filled")]
    fn reading_a_reserved_but_unfilled_slot_panics() {
        let mut arena = TypeArena::new();
        let reserved = arena.reserve();

        arena.get(reserved.id());
    }

    #[test]
    fn fill_makes_a_reserved_slot_readable() {
        let mut arena = TypeArena::new();
        let reserved = arena.reserve();
        let id = arena.fill(reserved, Type::NoPrefix);

        assert_eq!(id, reserved.id());
        assert_eq!(*arena.get(id), Type::NoPrefix);
    }

    /// Reproduces `docs/dotty-core-design.md` §8's `def head[A](xs: A): A`
    /// example (simplified to skip modeling `List[A]`) and asserts that a
    /// `ParamRef` resolved back through its `binder: TypeId` yields the exact
    /// `TypeParam` it was bound to — the guarantee `[BLOCKER 1]` exists for.
    #[test]
    fn param_ref_resolves_back_to_the_exact_type_param_it_was_bound_to() {
        let mut arena = TypeArena::new();

        let poly_binder = arena.reserve();
        let param_ref = arena.alloc(Type::ParamRef {
            binder: poly_binder.id(),
            index: 0,
        });

        let xs = MethodParam {
            name: TermName::new(NameId::new(1)),
            ty: param_ref,
            erased: false,
        };
        let method = arena.alloc(Type::Method(MethodType {
            params: vec![xs],
            result: param_ref,
            kind: MethodKind::Plain,
        }));

        let type_param_a = TypeParam {
            name: TypeName::new(NameId::new(2)),
            bounds: arena.alloc(Type::NoPrefix),
            variance: Variance::Invariant,
        };
        let poly = arena.fill(
            poly_binder,
            Type::Poly(PolyType {
                params: vec![type_param_a],
                result: method,
            }),
        );

        let &Type::ParamRef { binder, index } = arena.get(param_ref) else {
            panic!("expected a ParamRef");
        };
        assert_eq!(binder, poly);

        let Type::Poly(resolved_poly) = arena.get(binder) else {
            panic!("binder must resolve to the Poly it was reserved for");
        };
        assert_eq!(resolved_poly.params[index as usize], type_param_a);
    }
}
