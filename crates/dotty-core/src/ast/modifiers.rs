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
    /// Source private visibility, with an optional enclosing qualifier.
    Private {
        /// Optional enclosing access qualifier, including the special this name where supported.
        qualifier: Option<Name>,
    },
    /// Source protected visibility, with an optional enclosing qualifier.
    Protected {
        /// Optional enclosing access qualifier, including the special this name where supported.
        qualifier: Option<Name>,
    },
}

/// One source-level modifier keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Modifier {
    /// Parser-level distinction for a trait definition.
    ///
    /// This is kept in source metadata rather than inferred from constructor
    /// shape, because a trait and a class share the same `TypeDef`/`Template`
    /// tree family but have different source meaning.
    Trait,
    /// Parser-level distinction for an enum definition.
    ///
    /// Enums share the source-level `TypeDef`/`Template` shape with classes,
    /// so the marker preserves their identity without introducing a separate
    /// enum AST node.
    Enum,
    /// Parser-level distinction for a singleton enum case.
    ///
    /// This is generated from the `case` position inside an enum body rather
    /// than represented as a source modifier keyword. It keeps enum-case
    /// identity separate from the `Case` modifier used by case classes and
    /// case objects.
    EnumCase,
    /// Parser-level distinction for a `package object` source definition.
    ///
    /// Dotty represents this source form as a `ModuleDef` with its internal
    /// package flag set. This marker preserves that distinction without
    /// introducing a separate module AST node or performing desugaring.
    PackageObject,
    /// Parser-level role for a constructor parameter that contributes an
    /// accessor on its owning class.
    ///
    /// This is metadata synthesized from the constructor-parameter context,
    /// not a source modifier keyword.
    ParamAccessor,
    /// Parser-level role for a declared type parameter.
    Param,
    /// Parser-level role for a plain constructor parameter that is not an
    /// accessor. This must not be represented as written `private` visibility.
    PrivateLocal,
    /// Source definition produced by extension-method syntax.
    Extension,
    /// Source marker for an abstract declaration without a complete implementation.
    Abstract,
    /// Source marker for a declaration that cannot be overridden or extended.
    Final,
    /// Source marker for a declaration restricting direct subclasses.
    Sealed,
    /// Source marker for a case class or case member marker.
    Case,
    /// Source marker for a mutable variable or parameter accessor.
    Var,
    /// Source marker for an update modifier retained by the source parser.
    Update,
    /// Source marker for a legacy implicit declaration.
    Implicit,
    /// Source marker for a contextual given declaration.
    Given,
    /// Source marker for an impure function marker.
    Impure,
    /// Source marker for a lazily initialized value.
    Lazy,
    /// Source marker for an explicit override declaration.
    Override,
    /// Source marker for an inline definition.
    Inline,
    /// Source marker for a transparent inline definition.
    Transparent,
    /// Source marker for an opaque type alias.
    Opaque,
    /// Source marker for a class explicitly open to extension.
    Open,
    /// Source marker for a declaration usable with infix syntax.
    Infix,
    /// Source marker for a tracked parameter marker.
    Tracked,
    /// Source marker for an into type marker.
    Into,
    /// Source marker for a parameter or declaration erased from runtime execution.
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
    /// Source or semantic visibility of the declaration.
    pub visibility: Option<VisibilitySyntax>,
    /// Byte range of the written qualifier token in the declaration's source.
    pub visibility_qualifier_range: Option<crate::TextRange>,
    /// Source modifiers retained on this syntax node.
    pub modifiers: Vec<Modifier>,
    /// Annotations attached to this item, preserving their recorded order.
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
    fn constructor_parameter_roles_are_distinct_from_source_visibility() {
        assert_ne!(Modifier::ParamAccessor, Modifier::PrivateLocal);
        assert_ne!(Modifier::PrivateLocal, Modifier::Var);
    }

    #[test]
    fn type_parameter_role_is_distinct_from_constructor_accessor_role() {
        assert_ne!(Modifier::Param, Modifier::ParamAccessor);
        assert_ne!(Modifier::Param, Modifier::PrivateLocal);
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

    #[test]
    fn extension_is_a_distinct_source_modifier() {
        assert_ne!(Modifier::Extension, Modifier::Implicit);
        assert_ne!(Modifier::Extension, Modifier::Given);
    }

    #[test]
    fn trait_is_a_distinct_source_definition_modifier() {
        assert_ne!(Modifier::Trait, Modifier::Abstract);
        assert_ne!(Modifier::Trait, Modifier::Case);
    }

    #[test]
    fn enum_is_a_distinct_source_definition_modifier() {
        assert_ne!(Modifier::Enum, Modifier::Trait);
        assert_ne!(Modifier::Enum, Modifier::Case);
    }

    #[test]
    fn enum_case_is_distinct_from_case_definition_metadata() {
        assert_ne!(Modifier::EnumCase, Modifier::Case);
        assert_ne!(Modifier::EnumCase, Modifier::Enum);
    }

    #[test]
    fn package_object_is_a_distinct_parser_level_module_marker() {
        assert_ne!(Modifier::PackageObject, Modifier::EnumCase);
        assert_ne!(Modifier::PackageObject, Modifier::Case);
    }
}
