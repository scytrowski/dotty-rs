//! Source-level modifiers, distinct from semantic [`crate::symbols::SymbolFlags`].
//!
//! A parsed `override` keyword and a resolved "this symbol overrides a
//! parent member" fact are different things computed at different phases;
//! conflating them would force the namer to invent flags for syntax the
//! parser never saw, and vice versa.

use crate::ast::phase::Untyped;
use crate::ids::TreeId;
use crate::names::Name;

/// `private`/`private[q]`/`protected`/`protected[q]` as written, or absent
/// (Scala's default visibility). `q` is a bare [`Name`] rather than a
/// [`crate::names::TypeName`] because it may be the special qualifier
/// `this`, not a type name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibilitySyntax {
    Private { qualifier: Option<Name> },
    Protected { qualifier: Option<Name> },
}

/// One source-level modifier keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Modifier {
    Abstract,
    Final,
    Sealed,
    Case,
    Var,
    Update,
    Implicit,
    Given,
    Impure,
    Lazy,
    Override,
    Inline,
    Transparent,
    Opaque,
    Open,
    Infix,
    Tracked,
    Into,
    Erased,
}

/// The syntactic modifiers and annotations attached to a definition.
///
/// Only ever reachable from an untyped definition node (it is
/// `Untyped::DefMetadata`, and `Typed::DefMetadata = ()`), so
/// `annotations: Vec<TreeId<Untyped>>` never becomes a typed tree pointing
/// into an untyped arena — see `docs/dotty-core-design.md` §7, `[MAJOR 2]`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub visibility: Option<VisibilitySyntax>,
    pub modifiers: Vec<Modifier>,
    pub annotations: Vec<TreeId<Untyped>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_modifiers_have_no_visibility_or_modifiers() {
        let modifiers = Modifiers::default();

        assert_eq!(modifiers.visibility, None);
        assert!(modifiers.modifiers.is_empty());
        assert!(modifiers.annotations.is_empty());
    }

    #[test]
    fn private_and_protected_are_distinguishable_with_the_same_qualifier() {
        let qualifier = None;

        assert_ne!(
            VisibilitySyntax::Private { qualifier },
            VisibilitySyntax::Protected { qualifier }
        );
    }

    #[test]
    fn impure_is_a_distinct_function_type_modifier() {
        assert_ne!(Modifier::Impure, Modifier::Given);
    }

    #[test]
    fn var_is_a_distinct_mutability_modifier() {
        assert_ne!(Modifier::Var, Modifier::Lazy);
    }

    #[test]
    fn update_is_a_distinct_capture_checking_modifier() {
        assert_ne!(Modifier::Update, Modifier::Var);
    }

    #[test]
    fn tracked_is_a_distinct_capture_checking_modifier() {
        assert_ne!(Modifier::Tracked, Modifier::Into);
    }

    #[test]
    fn into_is_a_distinct_source_modifier() {
        assert_ne!(Modifier::Into, Modifier::Given);
    }
}
