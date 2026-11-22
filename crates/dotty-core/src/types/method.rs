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
    /// Interned name in the term namespace; resolve its text through the session interner.
    pub name: TermName,
    /// Parameter type in the same semantic arena as its enclosing method binder.
    pub ty: TypeId,
    /// Whether this parameter is erased from runtime argument passing.
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
    /// Ordinary value parameter clause.
    Plain,
    /// Legacy implicit parameter clause.
    Implicit,
    /// Contextual using parameter clause.
    Contextual,
}

/// `(params): result`, e.g. the method type of `def foo(x: Int): String`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodType {
    /// Value parameters in this clause, preserving declaration order.
    pub params: Vec<MethodParam>,
    /// Result type after this clause; a later parameter clause is represented by a nested callable
    /// type.
    pub result: TypeId,
    /// Whether this parameter clause is ordinary, implicit, or contextual.
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
    /// Interned name in the type namespace; resolve its text through the session interner.
    pub name: TypeName,
    /// Lower and upper semantic bounds in the enclosing binder’s type arena.
    pub bounds: TypeId,
    /// Explicit source variance, absent when no variance marker was written.
    pub declared_variance: Option<Variance>,
}

/// A declared variance.
///
/// There is deliberately no "unspecified" member: absence of a declaration is
/// `Option::None` on [`TypeParam::declared_variance`], not a variance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variance {
    /// Parameter appears invariantly in the declared type constructor.
    Invariant,
    /// Parameter is declared covariant with a plus marker.
    Covariant,
    /// Parameter is declared contravariant with a minus marker.
    Contravariant,
}

/// A canonical class-owned type parameter and its declaration-site variance.
/// Class parameter lists are ordered semantic metadata; keeping identity and
/// variance together prevents adapters from publishing parallel lists that
/// can drift out of alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassTypeParameter {
    /// Declaration identity involved in the operation, relative to the caller’s symbol table.
    pub symbol: crate::SymbolId,
    /// `None` means an adapter could authenticate the parameter identity but
    /// could not interpret its variance evidence. A supported absent class
    /// variance marker is represented as `Some(Variance::Invariant)`.
    pub variance: Option<Variance>,
}

/// `[params]: result`, e.g. the type of `def head[A](xs: List[A]): A`'s
/// leading type-parameter clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyType {
    /// Bound type parameters in declaration order; parameter references use these positions.
    pub params: Vec<TypeParam>,
    /// Result type in the same arena; further parameter clauses are represented by nested callable
    /// types.
    pub result: TypeId,
}

/// `[params] =>> result`, a type-level lambda such as
/// `[X] =>> Either[String, X]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeLambda {
    /// Bound type parameters in declaration order; parameter references use these positions.
    pub params: Vec<TypeParam>,
    /// Result type in the same arena; further parameter clauses are represented by nested callable
    /// types.
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
