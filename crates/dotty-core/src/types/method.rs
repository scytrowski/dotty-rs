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
    /// Whether this is a JVM `ACC_VARARGS` trailing array parameter (Java
    /// `T... xs`). Without this, `void f(String... xs)` and
    /// `void f(String[] xs)` would lower to the exact same `MethodParam` and
    /// become semantically indistinguishable, even though only the former
    /// permits call sites to pass loose trailing arguments.
    pub varargs: bool,
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
///
/// `declared_variance` is the variance the parameter was *declared* with, and
/// absence is a state of its own: `None` is "no declared variance", which is
/// what a standalone `[A] =>> A` or a `PolyType` has, while
/// `Some(Variance::Invariant)` is an explicit invariant declaration (a TASTy
/// `STABLE` marker). Dotty keeps the same distinction
/// (`HKTypeLambda.isDeclaredVarianceLambda = variances.nonEmpty`, where the
/// list may hold `Invariant`), so the two must not be conflated. It holds
/// declared variance only: variance inferred from a type's structure belongs
/// to a later typer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeParam {
    pub name: TypeName,
    pub bounds: TypeId,
    pub declared_variance: Option<Variance>,
}

/// A declared variance.
///
/// There is deliberately no "unspecified" member: absence of a declaration is
/// `Option::None` on [`TypeParam::declared_variance`], not a variance.
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
            varargs: false,
        };

        assert!(param.erased);
        assert_eq!(param.ty, TypeId::new(2));
    }

    #[test]
    fn varargs_and_a_plain_array_typed_param_are_distinguishable() {
        let array_param = MethodParam {
            name: TermName::new(NameId::new(1)),
            ty: TypeId::new(2),
            erased: false,
            varargs: false,
        };
        let varargs_param = MethodParam {
            varargs: true,
            ..array_param
        };

        assert_ne!(array_param, varargs_param);
        assert!(varargs_param.varargs);
        assert!(!array_param.varargs);
    }

    #[test]
    fn no_declared_variance_is_not_an_explicit_invariant_declaration() {
        let param = |declared_variance| TypeParam {
            name: TypeName::new(NameId::new(1)),
            bounds: TypeId::new(2),
            declared_variance,
        };

        assert_ne!(param(None), param(Some(Variance::Invariant)));
        assert_ne!(
            param(Some(Variance::Invariant)),
            param(Some(Variance::Covariant))
        );
    }

    #[test]
    fn type_param_carries_its_declared_variance() {
        let param = TypeParam {
            name: TypeName::new(NameId::new(1)),
            bounds: TypeId::new(2),
            declared_variance: Some(Variance::Covariant),
        };

        assert_eq!(param.declared_variance, Some(Variance::Covariant));
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
