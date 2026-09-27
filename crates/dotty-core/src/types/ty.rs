//! The semantic type model.

use crate::ids::{AnnotationId, NameId, SymbolId, TypeId};
use crate::names::{Name, TermName, TypeName};
use crate::types::class_info::ClassInfo;
use crate::types::constant::Constant;
use crate::types::method::{MethodType, PolyType, TypeLambda};

/// A type-checking failure recorded in place of a real type, so that one
/// error does not require aborting the rest of type checking.
///
/// `message` is an interned diagnostic string; the full diagnostic itself is
/// reported separately (`dotty-core` does not depend on a diagnostics crate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorType {
    pub message: NameId,
}

/// `[bound] scrutinee match { cases }`.
///
/// Each case is a type in the same arena, in source order, one of:
///
/// * a `Type::MatchCase { pattern, result }`, or
/// * a `Type::TypeLambda` whose result is a `Type::MatchCase`: Dotty's
///   `[X1, ..., Xn] =>> MatchCase(pattern, result)`, how a case that captures
///   type variables (`case Iterable[t] => t`) is written. The captures are that
///   lambda's parameters, named by ordinary `ParamRef`s; there is no
///   match-specific binder.
///
/// Either shape may sit behind sharing. Nothing checks the shape: a match type
/// is rebuilt as written, and reduction is not this model's concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchType {
    pub bound: TypeId,
    pub scrutinee: TypeId,
    pub cases: Vec<TypeId>,
}

/// What a [`Type::TypeRef`] designates: Dotty's `NamedType` designator,
/// `Symbol | Name`.
///
/// * `Symbol` is a stable declaration identity, used whenever a declaration
///   symbol exists (every direct, symbol, package, resolved and `REFin`
///   reference).
/// * `Name` is the selection `prefix.name` where the member has no `SymbolId`,
///   such as a member of a structural refinement. It is a real semantic
///   reference, not `Error`, `NoType`, a placeholder or an unresolved string.
///   It carries no copy of the member's info: the source of truth stays the
///   `Refined` graph, which [`crate::types::lookup_structural_member`] reads.
///
/// The two are different targets even when the symbol's text is the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRefTarget {
    Symbol(SymbolId),
    Name(TypeName),
}

/// What a [`Type::TermRef`] designates. See [`TypeRefTarget`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermRefTarget {
    Symbol(SymbolId),
    Name(TermName),
}

impl TypeRefTarget {
    /// The declaration symbol, or `None` for a name-designed target.
    pub const fn symbol(&self) -> Option<SymbolId> {
        match self {
            Self::Symbol(symbol) => Some(*symbol),
            Self::Name(_) => None,
        }
    }
}

impl TermRefTarget {
    /// The declaration symbol, or `None` for a name-designed target.
    pub const fn symbol(&self) -> Option<SymbolId> {
        match self {
            Self::Symbol(symbol) => Some(*symbol),
            Self::Name(_) => None,
        }
    }
}

/// The semantic type model.
///
/// `TermRef`/`TypeRef` designate a [`SymbolId`] when the declaration has one,
/// and a [`TermName`]/[`TypeName`] when it has none (see [`TypeRefTarget`]).
/// `ParamRef`/`RecThis` reference their binder via the binder's own
/// [`TypeId`] rather than a separate `BinderId` — see
/// `docs/dotty-core-design.md` §8, `[BLOCKER 1]`.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    NoType,
    Error(ErrorType),
    NoPrefix,

    TermRef {
        prefix: TypeId,
        target: TermRefTarget,
    },
    TypeRef {
        prefix: TypeId,
        target: TypeRefTarget,
    },

    /// `C.this`. `class` is a class, trait or module class, or a package:
    /// the core keeps one `SymbolKind::Package` symbol per package, shared by
    /// `TYPEREFpkg` and `TERMREFpkg` (see [`crate::packages`]), so `this` of a
    /// package names that one symbol.
    ThisType {
        class: SymbolId,
    },
    SuperType {
        this_type: TypeId,
        super_type: TypeId,
    },

    Constant(Constant),

    Applied {
        tycon: TypeId,
        args: Vec<TypeId>,
    },
    /// Genuine lower/upper bounds (`>: low <: high`).
    Bounds {
        low: TypeId,
        high: TypeId,
    },
    /// The info of a type alias or opaque-free alias member (`= alias`).
    ///
    /// Dotty's `AliasingBounds` is bounds-like internally, but the semantic
    /// distinction survives: this is not `Bounds { low: alias, high: alias }`.
    /// It is unrelated to `SymbolKind::TypeAlias`, which classifies a symbol.
    AliasingBounds {
        alias: TypeId,
    },
    ByName {
        result: TypeId,
    },
    /// A flexible type (`FlexibleType(hi)` in Dotty), the explicit-nulls type
    /// of a Java-defined member: its members are those of `underlying`, but
    /// the wrapper is part of the type and is never stripped by the model.
    Flexible {
        underlying: TypeId,
    },

    And {
        left: TypeId,
        right: TypeId,
    },
    Or {
        left: TypeId,
        right: TypeId,
    },

    Refined {
        parent: TypeId,
        name: Name,
        info: TypeId,
    },

    /// `parent` may itself contain `RecThis { binder }` values where
    /// `binder` is the `TypeId` this very value is stored under.
    Recursive {
        parent: TypeId,
    },
    RecThis {
        binder: TypeId,
    },

    Method(MethodType),
    Poly(PolyType),
    TypeLambda(TypeLambda),

    /// `binder` is the `TypeId` of the enclosing `Method`/`Poly`/`TypeLambda`
    /// value itself.
    ParamRef {
        binder: TypeId,
        index: u32,
    },

    Match(MatchType),
    MatchCase {
        pattern: TypeId,
        result: TypeId,
    },

    Annotated {
        underlying: TypeId,
        annotation: AnnotationId,
    },

    Wildcard {
        bounds: TypeId,
    },

    JavaArray {
        element: TypeId,
    },

    /// A Scala repeated parameter's sequence-shaped value type.
    ///
    /// This preserves the repeated marker and element type for references to
    /// the parameter inside its method body. The owning method signature
    /// stores the element type separately with `MethodParam::varargs`; this
    /// wrapper does not claim a particular library `Seq` class identity.
    Repeated {
        element: TypeId,
    },

    ClassInfo(ClassInfo),
}

impl Type {
    /// A `TypeRef` to the declaration `symbol`.
    pub const fn type_ref(prefix: TypeId, symbol: SymbolId) -> Self {
        Self::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        }
    }

    /// A `TermRef` to the declaration `symbol`.
    pub const fn term_ref(prefix: TypeId, symbol: SymbolId) -> Self {
        Self::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        }
    }

    /// The declaration symbol of a symbol-designated `TypeRef` / `TermRef`.
    /// `None` for a name-designated reference and for every other type.
    pub const fn reference_symbol(&self) -> Option<SymbolId> {
        match self {
            Self::TypeRef { target, .. } => target.symbol(),
            Self::TermRef { target, .. } => target.symbol(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ScopeId;
    use crate::names::Namespace;
    use crate::types::method::{MethodKind, TypeParam};

    fn name(raw: u32) -> Name {
        Name::new(NameId::new(raw), Namespace::Term)
    }

    #[test]
    fn no_type_and_no_prefix_are_distinct_unit_variants() {
        assert_ne!(Type::NoType, Type::NoPrefix);
    }

    #[test]
    fn error_carries_an_interned_message() {
        let ty = Type::Error(ErrorType {
            message: NameId::new(1),
        });

        assert_eq!(
            ty,
            Type::Error(ErrorType {
                message: NameId::new(1)
            })
        );
    }

    #[test]
    fn term_ref_and_type_ref_carry_a_prefix_and_symbol() {
        let prefix = TypeId::new(1);
        let symbol = SymbolId::new(2);

        assert_ne!(
            Type::term_ref(prefix, symbol),
            Type::type_ref(prefix, symbol)
        );
        assert_eq!(
            Type::type_ref(prefix, symbol).reference_symbol(),
            Some(symbol)
        );
    }

    #[test]
    fn a_name_target_is_not_a_symbol_target_even_when_the_text_matches() {
        let prefix = TypeId::new(1);
        let text = NameId::new(2);
        let by_name = Type::TypeRef {
            prefix,
            target: TypeRefTarget::Name(TypeName::new(text)),
        };

        assert_ne!(by_name, Type::type_ref(prefix, SymbolId::new(2)));
        assert_eq!(by_name.reference_symbol(), None);
        assert_ne!(
            TypeRefTarget::Name(TypeName::new(text)),
            TypeRefTarget::Name(TypeName::new(NameId::new(3)))
        );
        let term = Type::TermRef {
            prefix,
            target: TermRefTarget::Name(TermName::new(text)),
        };
        assert_eq!(term.reference_symbol(), None);
        assert_ne!(term, by_name);
    }

    #[test]
    fn this_type_and_super_type_carry_their_classes() {
        let this = Type::ThisType {
            class: SymbolId::new(1),
        };
        let sup = Type::SuperType {
            this_type: TypeId::new(1),
            super_type: TypeId::new(2),
        };

        assert_ne!(this, sup);
    }

    #[test]
    fn constant_wraps_a_constant_value() {
        let ty = Type::Constant(Constant::Int(42));

        assert_eq!(ty, Type::Constant(Constant::Int(42)));
    }

    #[test]
    fn applied_carries_a_tycon_and_argument_list() {
        let ty = Type::Applied {
            tycon: TypeId::new(1),
            args: vec![TypeId::new(2), TypeId::new(3)],
        };

        assert_eq!(
            ty,
            Type::Applied {
                tycon: TypeId::new(1),
                args: vec![TypeId::new(2), TypeId::new(3)],
            }
        );
    }

    #[test]
    fn bounds_and_by_name_carry_their_operand_types() {
        let bounds = Type::Bounds {
            low: TypeId::new(1),
            high: TypeId::new(2),
        };
        let by_name = Type::ByName {
            result: TypeId::new(3),
        };

        assert_ne!(bounds, by_name);
    }

    #[test]
    fn aliasing_bounds_is_not_two_sided_bounds_with_equal_ends() {
        let alias = TypeId::new(1);

        assert_ne!(
            Type::AliasingBounds { alias },
            Type::Bounds {
                low: alias,
                high: alias,
            }
        );
    }

    #[test]
    fn and_and_or_are_distinguishable_despite_sharing_shape() {
        let and = Type::And {
            left: TypeId::new(1),
            right: TypeId::new(2),
        };
        let or = Type::Or {
            left: TypeId::new(1),
            right: TypeId::new(2),
        };

        assert_ne!(and, or);
    }

    #[test]
    fn refined_carries_a_parent_name_and_member_info() {
        let ty = Type::Refined {
            parent: TypeId::new(1),
            name: name(2),
            info: TypeId::new(3),
        };

        assert_eq!(
            ty,
            Type::Refined {
                parent: TypeId::new(1),
                name: name(2),
                info: TypeId::new(3),
            }
        );
    }

    #[test]
    fn recursive_and_rec_this_reference_a_binder_type_id() {
        let recursive = Type::Recursive {
            parent: TypeId::new(1),
        };
        let rec_this = Type::RecThis {
            binder: TypeId::new(1),
        };

        assert_ne!(recursive, rec_this);
    }

    #[test]
    fn method_poly_and_type_lambda_wrap_their_payload_structs() {
        let method = Type::Method(MethodType {
            params: vec![],
            result: TypeId::new(1),
            kind: MethodKind::Plain,
        });
        let poly = Type::Poly(PolyType {
            params: vec![],
            result: TypeId::new(1),
        });
        let lambda = Type::TypeLambda(TypeLambda {
            params: vec![],
            result: TypeId::new(1),
        });

        assert_ne!(method, poly);
        assert_ne!(poly, lambda);
    }

    #[test]
    fn param_ref_identifies_a_binder_and_index() {
        let ty = Type::ParamRef {
            binder: TypeId::new(1),
            index: 0,
        };

        assert_eq!(
            ty,
            Type::ParamRef {
                binder: TypeId::new(1),
                index: 0
            }
        );
        assert_ne!(
            ty,
            Type::ParamRef {
                binder: TypeId::new(1),
                index: 1
            }
        );
    }

    #[test]
    fn match_and_match_case_carry_their_operand_types() {
        let match_ty = Type::Match(MatchType {
            bound: TypeId::new(1),
            scrutinee: TypeId::new(2),
            cases: vec![TypeId::new(3)],
        });
        let case = Type::MatchCase {
            pattern: TypeId::new(4),
            result: TypeId::new(5),
        };

        assert_ne!(match_ty, case);
    }

    #[test]
    fn annotated_carries_an_underlying_type_and_annotation() {
        let ty = Type::Annotated {
            underlying: TypeId::new(1),
            annotation: AnnotationId::new(2),
        };

        assert_eq!(
            ty,
            Type::Annotated {
                underlying: TypeId::new(1),
                annotation: AnnotationId::new(2),
            }
        );
    }

    #[test]
    fn wildcard_and_java_array_carry_their_operand_type() {
        let wildcard = Type::Wildcard {
            bounds: TypeId::new(1),
        };
        let array = Type::JavaArray {
            element: TypeId::new(1),
        };

        assert_ne!(wildcard, array);
    }

    #[test]
    fn repeated_parameter_type_retains_its_element_type() {
        let repeated = Type::Repeated {
            element: TypeId::new(4),
        };

        assert_eq!(
            repeated,
            Type::Repeated {
                element: TypeId::new(4)
            }
        );
    }

    #[test]
    fn class_info_wraps_its_payload_struct() {
        let ty = Type::ClassInfo(ClassInfo {
            prefix: TypeId::new(1),
            class: SymbolId::new(2),
            parents: vec![],
            declarations: ScopeId::new(3),
            self_type: None,
        });

        assert_eq!(
            ty,
            Type::ClassInfo(ClassInfo {
                prefix: TypeId::new(1),
                class: SymbolId::new(2),
                parents: vec![],
                declarations: ScopeId::new(3),
                self_type: None,
            })
        );
    }

    #[test]
    fn type_param_can_appear_in_poly_and_type_lambda_params() {
        let param = TypeParam {
            name: crate::names::TypeName::new(NameId::new(1)),
            bounds: TypeId::new(2),
            declared_variance: None,
        };
        let poly = PolyType {
            params: vec![param],
            result: TypeId::new(3),
        };

        assert_eq!(poly.params.len(), 1);
    }
}
