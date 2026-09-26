//! Shared conversion from an ordinary type to the semantic info of a type
//! alias.

use super::{Type, TypeArena};
use crate::TypeId;

/// A semantic type that cannot be used as a type alias RHS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToBoundsError {
    /// By-name types are method parameter types, not alias RHS types.
    ByName,
    /// Method and polymorphic types are method signatures, not alias RHSs.
    Methodic,
}

/// Converts a projected alias RHS to semantic alias bounds.
///
/// Genuine bounds and already aliased bounds keep their existing identity.
/// Ordinary types receive a fresh [`Type::AliasingBounds`] wrapper. This
/// operation is independent of the source or TASTy representation that
/// produced `ty`.
pub fn to_bounds(types: &mut TypeArena, ty: TypeId) -> Result<TypeId, ToBoundsError> {
    match types.get(ty) {
        Type::Bounds { .. } | Type::AliasingBounds { .. } => Ok(ty),
        Type::ByName { .. } => Err(ToBoundsError::ByName),
        Type::Method(_) | Type::Poly(_) => Err(ToBoundsError::Methodic),
        _ => Ok(types.alloc(Type::AliasingBounds { alias: ty })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_existing_bounds_identity() {
        let mut types = TypeArena::new();
        let low = types.alloc(Type::NoType);
        let high = types.alloc(Type::NoType);
        let bounds = types.alloc(Type::Bounds { low, high });

        assert_eq!(to_bounds(&mut types, bounds), Ok(bounds));
    }

    #[test]
    fn preserves_existing_aliasing_bounds_identity() {
        let mut types = TypeArena::new();
        let alias = types.alloc(Type::NoType);
        let bounds = types.alloc(Type::AliasingBounds { alias });

        assert_eq!(to_bounds(&mut types, bounds), Ok(bounds));
    }

    #[test]
    fn wraps_an_ordinary_type_as_aliasing_bounds() {
        let mut types = TypeArena::new();
        let ty = types.alloc(Type::NoType);

        let bounds = to_bounds(&mut types, ty).unwrap();

        assert!(matches!(types.get(bounds), Type::AliasingBounds { alias } if *alias == ty));
    }

    #[test]
    fn rejects_a_by_name_type() {
        let mut types = TypeArena::new();
        let result = types.alloc(Type::NoType);
        let by_name = types.alloc(Type::ByName { result });

        assert_eq!(to_bounds(&mut types, by_name), Err(ToBoundsError::ByName));
    }

    #[test]
    fn rejects_a_method_type() {
        let mut types = TypeArena::new();
        let result = types.alloc(Type::NoType);
        let method = types.alloc(Type::Method(crate::types::MethodType {
            params: Vec::new(),
            result,
            kind: crate::types::MethodKind::Plain,
        }));

        assert_eq!(to_bounds(&mut types, method), Err(ToBoundsError::Methodic));
    }

    #[test]
    fn rejects_a_polymorphic_type() {
        let mut types = TypeArena::new();
        let result = types.alloc(Type::NoType);
        let poly = types.alloc(Type::Poly(crate::types::PolyType {
            params: Vec::new(),
            result,
        }));

        assert_eq!(to_bounds(&mut types, poly), Err(ToBoundsError::Methodic));
    }
}
