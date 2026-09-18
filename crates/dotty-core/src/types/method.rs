//! Method, polymorphic, and type-lambda binder payloads.
//!
//! None of these carry their own `binder`/`id` field: once one is allocated
//! in a `TypeArena` as `Type::Method`/`Type::Poly`/`Type::TypeLambda`, the
//! `TypeId` it is stored under *is* its binder identity — see
//! `docs/dotty-core-design.md` §8, `[BLOCKER 1]`.

use crate::ids::TypeId;
use crate::names::{TermName, TypeName};

/// A term parameter of a [`MethodType`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodParam {
    pub name: TermName,
    pub ty: TypeId,
    pub erased: bool,
}

/// How a [`MethodType`]'s parameter clause binds its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Plain,
    Implicit,
    Contextual,
}

/// `(params): result`, e.g. the method type of `def foo(x: Int): String`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodType {
    pub params: Vec<MethodParam>,
    pub result: TypeId,
    pub kind: MethodKind,
}

/// A type parameter of a [`PolyType`] or [`TypeLambda`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeParam {
    pub name: TypeName,
    pub bounds: TypeId,
    pub variance: Variance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variance {
    Invariant,
    Covariant,
    Contravariant,
}

/// `[params]: result`, e.g. the type of `def head[A](xs: List[A]): A`'s
/// leading type-parameter clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyType {
    pub params: Vec<TypeParam>,
    pub result: TypeId,
}

/// `[params] =>> result`, a type-level lambda such as
/// `[X] =>> Either[String, X]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeLambda {
    pub params: Vec<TypeParam>,
    pub result: TypeId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NameId;

    #[test]
    fn method_param_carries_its_name_type_and_erasure() {
        let param = MethodParam {
            name: TermName::new(NameId::new(1)),
            ty: TypeId::new(2),
            erased: true,
        };

        assert!(param.erased);
        assert_eq!(param.ty, TypeId::new(2));
    }

    #[test]
    fn type_param_carries_its_variance() {
        let param = TypeParam {
            name: TypeName::new(NameId::new(1)),
            bounds: TypeId::new(2),
            variance: Variance::Covariant,
        };

        assert_eq!(param.variance, Variance::Covariant);
    }

    #[test]
    fn method_type_and_poly_type_are_distinguishable_by_kind_and_params() {
        let method = MethodType {
            params: vec![],
            result: TypeId::new(1),
            kind: MethodKind::Contextual,
        };
        let poly = PolyType {
            params: vec![],
            result: TypeId::new(1),
        };

        assert_eq!(method.kind, MethodKind::Contextual);
        assert!(poly.params.is_empty());
    }
}
