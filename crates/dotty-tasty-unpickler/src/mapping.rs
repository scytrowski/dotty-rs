//! Pure mappings from TASTy definition syntax to `dotty-core` symbol facts.
//!
//! Nothing here touches a `SemanticStore` or decodes a type: it turns the
//! modifiers, tags and names of a definition node into a namespace, flags,
//! visibility and a [`SymbolKind`]. Only facts that TASTy states directly are
//! mapped; a modifier that needs semantic interpretation (variance, accessor
//! roles, ...) is left for the milestone that owns it and listed below.
//!
//! Modifiers deliberately **not** mapped yet: `ENUM`, `ARTIFACT`,
//! `INLINEPROXY`, `MACRO`, `EXPORTED`, `OPEN`, `INFIX`, `INVISIBLE`,
//! `TRACKED`, `INTO` (no matching core flag), `COVARIANT`/`CONTRAVARIANT`
//! (variance belongs to type-parameter completion), and the accessor roles
//! `FIELDACCESSOR`, `CASEACCESSOR`, `PARAMSETTER`, `PARAMALIAS`,
//! `HASDEFAULT`, `STABLE`. Annotations are not read here: `from_tail` skips
//! `DefinitionTail::Annotation` entirely, including for the wire position it
//! carries, because that position is relative to the definition's own
//! payload, not an absolute AST address (`RawNode::reader` always starts a
//! fresh reader at offset 0) — the addresses pass 1 indexes for Milestone 5e1
//! come from the AST's own child edges instead (`enter::annotation_children`),
//! exactly like every other structural child a definition's tail is not.

use dotty_core::names::Namespace;
use dotty_core::symbols::{SymbolFlags, SymbolKind, Visibility};
use dotty_tasty::tasty::{
    ABSTRACT_TAG, CASE_TAG, DEFDEF_TAG, DefinitionTail, ERASED_TAG, EXTENSION_TAG, FINAL_TAG,
    GIVEN_TAG, IMPLICIT_TAG, INLINE_TAG, LAZY_TAG, LOCAL_TAG, MUTABLE_TAG, OBJECT_TAG, OPAQUE_TAG,
    OVERRIDE_TAG, PACKAGE_TAG, PARAM_TAG, PRIVATE_TAG, PROTECTED_TAG, PROTECTEDQUALIFIED_TAG,
    RawTree, SEALED_TAG, SHAREDTYPE_TAG, STATIC_TAG, SYNTHETIC_TAG, TRAIT_TAG, TRANSPARENT_TAG,
    TYPEDEF_TAG, TYPEPARAM_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TermValue, VALDEF_TAG,
};

use crate::error::UnpickleError;

/// The name TASTy gives every constructor.
pub(crate) const CONSTRUCTOR_NAME: &str = "<init>";

/// The name Dotty gives a `REFINEDtpt`'s synthetic refinement class
/// (`tpnme.REFINE_CLASS`, Milestone 5d2b).
pub(crate) const REFINEMENT_CLASS_NAME: &str = "<refinement>";

/// What the modifier list of a definition says about its symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclaredModifiers {
    pub flags: SymbolFlags,
    /// `Public` when no access modifier is present. TASTy never yields
    /// `Visibility::Package`: unqualified Scala access is public, private or
    /// protected.
    pub visibility: Visibility,
    /// `OBJECT`: a module (on a `VALDEF`) or its module class (on a
    /// `TYPEDEF`).
    pub is_object: bool,
    /// `TRAIT`.
    pub is_trait: bool,
    /// `LOCAL`: the object-private marker, as in `private[this]`.
    pub is_local: bool,
    /// A `private[Q]` / `protected[Q]` modifier. `visibility` stays `Public`
    /// in that case: the qualifier can only be resolved to a symbol once the
    /// definition's own symbol exists, so the enter pass sets the final
    /// visibility then.
    pub qualified: Option<QualifiedAccess>,
}

/// A qualified access modifier whose qualifier is not resolved yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QualifiedAccess {
    /// `protected[Q]` rather than `private[Q]`.
    pub protected: bool,
    pub qualifier: QualifierRef,
}

/// The qualifier `Q` of `private[Q]`, as far as the modifier itself says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QualifierRef {
    /// `TYPEREFpkg`: a package named directly (zero-based name reference).
    Package(u32),
    /// `SHAREDtype`: a tree shared elsewhere in the AST, by address.
    Shared(u32),
    /// `TYPEREFsymbol`: an enclosing definition, by the address of its node.
    Symbol(u32),
}

impl DeclaredModifiers {
    /// A definition with no modifiers.
    pub const NONE: Self = Self {
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        is_object: false,
        is_trait: false,
        is_local: false,
        qualified: None,
    };

    /// Reads the modifiers out of a definition's tail.
    ///
    /// A qualified access modifier is recorded in `qualified`, never widened
    /// or narrowed to a plain visibility. A qualifier whose tree shape is not
    /// a package name or a shared reference is
    /// `UnpickleError::UnsupportedQualifier`.
    pub fn from_tail(tail: &[DefinitionTail<'_>]) -> Result<Self, UnpickleError> {
        let mut modifiers = Self::NONE;
        let mut private = false;
        let mut protected = false;

        for entry in tail {
            match entry {
                DefinitionTail::Modifier(tag) => match *tag {
                    PRIVATE_TAG => private = true,
                    PROTECTED_TAG => protected = true,
                    OBJECT_TAG => modifiers.is_object = true,
                    TRAIT_TAG => modifiers.is_trait = true,
                    LOCAL_TAG => modifiers.is_local = true,
                    tag => {
                        if let Some(flag) = flag_for(tag) {
                            modifiers.flags = modifiers.flags | flag;
                        }
                    }
                },
                DefinitionTail::QualifiedModifier(modifier) => {
                    modifiers.qualified = Some(QualifiedAccess {
                        protected: modifier.tag == PROTECTEDQUALIFIED_TAG,
                        qualifier: qualifier_ref(&modifier.child)?,
                    });
                }
                DefinitionTail::Annotation(_) => {}
            }
        }

        // A definition is never both; if malformed input says so, the more
        // restrictive access wins.
        modifiers.visibility = if private {
            Visibility::Private
        } else if protected {
            Visibility::Protected
        } else {
            Visibility::Public
        };
        Ok(modifiers)
    }

    /// `private[this]`: private and object-private at once.
    pub fn is_private_this(&self) -> bool {
        self.visibility == Visibility::Private && self.is_local
    }
}

/// Reads the qualifier tree of a qualified access modifier.
fn qualifier_ref(tree: &RawTree<'_>) -> Result<QualifierRef, UnpickleError> {
    match tree {
        RawTree::Leaf(term) => match (term.tag, &term.value) {
            (TYPEREFPKG_TAG, TermValue::NameRef(name)) => Ok(QualifierRef::Package(*name)),
            (SHAREDTYPE_TAG, TermValue::AstRef(address)) => Ok(QualifierRef::Shared(*address)),
            (tag, _) => Err(UnpickleError::UnsupportedQualifier { tag }),
        },
        RawTree::NatAst {
            tag: TYPEREFSYMBOL_TAG,
            value,
            ..
        } => Ok(QualifierRef::Symbol(*value)),
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => {
            Err(UnpickleError::UnsupportedQualifier { tag: *tag })
        }
        RawTree::LengthNode(node) => Err(UnpickleError::UnsupportedQualifier { tag: node.tag }),
    }
}

/// The core flag a modifier tag maps to, if it has one.
fn flag_for(tag: u8) -> Option<SymbolFlags> {
    Some(match tag {
        ABSTRACT_TAG => SymbolFlags::ABSTRACT,
        FINAL_TAG => SymbolFlags::FINAL,
        SEALED_TAG => SymbolFlags::SEALED,
        CASE_TAG => SymbolFlags::CASE,
        IMPLICIT_TAG => SymbolFlags::IMPLICIT,
        GIVEN_TAG => SymbolFlags::GIVEN,
        LAZY_TAG => SymbolFlags::LAZY,
        MUTABLE_TAG => SymbolFlags::MUTABLE,
        INLINE_TAG => SymbolFlags::INLINE,
        TRANSPARENT_TAG => SymbolFlags::TRANSPARENT,
        OPAQUE_TAG => SymbolFlags::OPAQUE,
        EXTENSION_TAG => SymbolFlags::EXTENSION,
        STATIC_TAG => SymbolFlags::STATIC,
        SYNTHETIC_TAG => SymbolFlags::SYNTHETIC,
        ERASED_TAG => SymbolFlags::ERASED,
        OVERRIDE_TAG => SymbolFlags::OVERRIDE,
        _ => return None,
    })
}

/// The namespace of the name a definition node declares, or `None` for a tag
/// that is not a definition.
///
/// Packages are term names in Scala. A module is a term (`VALDEF`) plus a
/// type (its `TYPEDEF` module class), so the two never share an identity.
pub(crate) fn namespace_of(definition_tag: u8) -> Option<Namespace> {
    match definition_tag {
        PACKAGE_TAG | VALDEF_TAG | DEFDEF_TAG | PARAM_TAG => Some(Namespace::Term),
        TYPEDEF_TAG | TYPEPARAM_TAG => Some(Namespace::Type),
        _ => None,
    }
}

/// The kind of a `TYPEDEF`.
///
/// A definition with a template is a class, a trait, or (with `OBJECT`) the
/// module class of an object. Without a template it is a type member, which
/// `dotty-core` models as [`SymbolKind::TypeAlias`] whether it is an alias,
/// an opaque type, or an abstract type with bounds: there is no separate
/// abstract-type kind yet, and the bounds themselves are read by a later
/// milestone.
pub(crate) fn type_def_kind(modifiers: &DeclaredModifiers, has_template: bool) -> SymbolKind {
    match (has_template, modifiers.is_object, modifiers.is_trait) {
        (false, ..) => SymbolKind::TypeAlias,
        (true, true, _) => SymbolKind::ModuleClass,
        (true, false, true) => SymbolKind::Trait,
        (true, false, false) => SymbolKind::Class,
    }
}

/// The kind of a `VALDEF`.
///
/// `owner_kind` is the kind of the enclosing definition. A module is
/// [`SymbolKind::Object`]; a `val`/`var` directly inside a class-like owner
/// is a [`SymbolKind::Field`] (mutability is the `MUTABLE` flag, matching how
/// `.class` fields are entered); anywhere else it is a
/// [`SymbolKind::Local`].
pub(crate) fn val_def_kind(modifiers: &DeclaredModifiers, owner_kind: SymbolKind) -> SymbolKind {
    if modifiers.is_object {
        SymbolKind::Object
    } else if is_class_like(owner_kind) {
        SymbolKind::Field
    } else {
        SymbolKind::Local
    }
}

/// The kind of a `DEFDEF`, from its rendered (signature-free) name.
pub(crate) fn def_def_kind(name: &str) -> SymbolKind {
    if name == CONSTRUCTOR_NAME {
        SymbolKind::Constructor
    } else {
        SymbolKind::Method
    }
}

/// The kind of a term `PARAM`.
///
/// A parameter in a class's template header is also a member — a `val` or
/// `var` parameter — unless it is `private[this]`, which is a plain
/// constructor parameter that declares no member. Any other parameter (of a
/// method, in a `DEFDEF`) is a [`SymbolKind::Parameter`].
pub(crate) fn term_param_kind(
    modifiers: &DeclaredModifiers,
    in_class_template: bool,
) -> SymbolKind {
    if in_class_template && !modifiers.is_private_this() {
        SymbolKind::Field
    } else {
        SymbolKind::Parameter
    }
}

/// The kind of a type `PARAM`. Always [`SymbolKind::TypeParameter`].
pub(crate) fn type_param_kind() -> SymbolKind {
    SymbolKind::TypeParameter
}

fn is_class_like(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Trait | SymbolKind::Object | SymbolKind::ModuleClass
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::{AstChildNode, PRIVATEQUALIFIED_TAG, SimpleTerm, TERMREFPKG_TAG};

    fn tail(tags: &[u8]) -> Vec<DefinitionTail<'static>> {
        tags.iter().copied().map(DefinitionTail::Modifier).collect()
    }

    fn modifiers(tags: &[u8]) -> DeclaredModifiers {
        DeclaredModifiers::from_tail(&tail(tags)).unwrap()
    }

    // --- namespaces ---

    #[test]
    fn term_definitions_and_packages_are_term_names() {
        for tag in [PACKAGE_TAG, VALDEF_TAG, DEFDEF_TAG, PARAM_TAG] {
            assert_eq!(namespace_of(tag), Some(Namespace::Term), "tag {tag}");
        }
    }

    #[test]
    fn type_definitions_and_type_parameters_are_type_names() {
        for tag in [TYPEDEF_TAG, TYPEPARAM_TAG] {
            assert_eq!(namespace_of(tag), Some(Namespace::Type), "tag {tag}");
        }
    }

    #[test]
    fn a_tag_that_is_not_a_definition_has_no_namespace() {
        assert_eq!(namespace_of(136), None);
    }

    // --- visibility ---

    #[test]
    fn no_access_modifier_is_public() {
        assert_eq!(modifiers(&[]).visibility, Visibility::Public);
    }

    #[test]
    fn private_is_private() {
        assert_eq!(modifiers(&[PRIVATE_TAG]).visibility, Visibility::Private);
    }

    #[test]
    fn protected_is_protected() {
        assert_eq!(
            modifiers(&[PROTECTED_TAG]).visibility,
            Visibility::Protected
        );
    }

    #[test]
    fn private_wins_over_protected_in_malformed_input() {
        assert_eq!(
            modifiers(&[PROTECTED_TAG, PRIVATE_TAG]).visibility,
            Visibility::Private
        );
    }

    #[test]
    fn local_alone_does_not_change_visibility() {
        let modifiers = modifiers(&[LOCAL_TAG]);

        assert_eq!(modifiers.visibility, Visibility::Public);
        assert!(modifiers.is_local);
    }

    #[test]
    fn private_and_local_together_are_private_this() {
        assert!(modifiers(&[PRIVATE_TAG, LOCAL_TAG]).is_private_this());
    }

    #[test]
    fn private_without_local_is_not_private_this() {
        assert!(!modifiers(&[PRIVATE_TAG]).is_private_this());
    }

    fn qualified(tag: u8, leaf_tag: u8, value: TermValue) -> DefinitionTail<'static> {
        DefinitionTail::QualifiedModifier(AstChildNode {
            tag,
            child: RawTree::Leaf(SimpleTerm::new(leaf_tag, value).unwrap()),
            offset: 0,
        })
    }

    #[test]
    fn private_with_a_package_qualifier_is_recorded_unresolved() {
        let tail = [qualified(
            PRIVATEQUALIFIED_TAG,
            TYPEREFPKG_TAG,
            TermValue::NameRef(7),
        )];

        let modifiers = DeclaredModifiers::from_tail(&tail).unwrap();

        assert_eq!(
            modifiers.qualified,
            Some(QualifiedAccess {
                protected: false,
                qualifier: QualifierRef::Package(7)
            })
        );
        assert_eq!(modifiers.visibility, Visibility::Public);
    }

    #[test]
    fn protected_with_a_shared_qualifier_is_recorded_unresolved() {
        let tail = [qualified(
            PROTECTEDQUALIFIED_TAG,
            SHAREDTYPE_TAG,
            TermValue::AstRef(21),
        )];

        let modifiers = DeclaredModifiers::from_tail(&tail).unwrap();

        assert_eq!(
            modifiers.qualified,
            Some(QualifiedAccess {
                protected: true,
                qualifier: QualifierRef::Shared(21)
            })
        );
    }

    #[test]
    fn a_direct_symbol_reference_qualifier_names_the_definition_address() {
        let tail = [DefinitionTail::QualifiedModifier(AstChildNode {
            tag: PRIVATEQUALIFIED_TAG,
            child: RawTree::NatAst {
                tag: TYPEREFSYMBOL_TAG,
                offset: 0,
                value: 4,
                child: Box::new(RawTree::Leaf(
                    SimpleTerm::new(TERMREFPKG_TAG, TermValue::NameRef(1)).unwrap(),
                )),
            },
            offset: 0,
        })];

        assert_eq!(
            DeclaredModifiers::from_tail(&tail).unwrap().qualified,
            Some(QualifiedAccess {
                protected: false,
                qualifier: QualifierRef::Symbol(4)
            })
        );
    }

    #[test]
    fn no_qualified_modifier_records_no_qualifier() {
        assert_eq!(modifiers(&[PRIVATE_TAG]).qualified, None);
    }

    #[test]
    fn a_qualifier_of_another_leaf_shape_is_unsupported() {
        let tail = [qualified(
            PRIVATEQUALIFIED_TAG,
            TERMREFPKG_TAG,
            TermValue::NameRef(1),
        )];

        assert_eq!(
            DeclaredModifiers::from_tail(&tail),
            Err(UnpickleError::UnsupportedQualifier {
                tag: TERMREFPKG_TAG
            })
        );
    }

    #[test]
    fn a_qualifier_that_is_a_wrapped_tree_is_unsupported() {
        let tail = [DefinitionTail::QualifiedModifier(AstChildNode {
            tag: PRIVATEQUALIFIED_TAG,
            child: RawTree::Ast {
                tag: 90,
                offset: 0,
                child: Box::new(RawTree::Leaf(
                    SimpleTerm::new(TYPEREFPKG_TAG, TermValue::NameRef(1)).unwrap(),
                )),
            },
            offset: 0,
        })];

        assert_eq!(
            DeclaredModifiers::from_tail(&tail),
            Err(UnpickleError::UnsupportedQualifier { tag: 90 })
        );
    }

    // --- flags ---

    #[test]
    fn each_directly_mapped_modifier_sets_exactly_its_flag() {
        let cases = [
            (ABSTRACT_TAG, SymbolFlags::ABSTRACT),
            (FINAL_TAG, SymbolFlags::FINAL),
            (SEALED_TAG, SymbolFlags::SEALED),
            (CASE_TAG, SymbolFlags::CASE),
            (IMPLICIT_TAG, SymbolFlags::IMPLICIT),
            (GIVEN_TAG, SymbolFlags::GIVEN),
            (LAZY_TAG, SymbolFlags::LAZY),
            (MUTABLE_TAG, SymbolFlags::MUTABLE),
            (INLINE_TAG, SymbolFlags::INLINE),
            (TRANSPARENT_TAG, SymbolFlags::TRANSPARENT),
            (OPAQUE_TAG, SymbolFlags::OPAQUE),
            (EXTENSION_TAG, SymbolFlags::EXTENSION),
            (STATIC_TAG, SymbolFlags::STATIC),
            (SYNTHETIC_TAG, SymbolFlags::SYNTHETIC),
            (ERASED_TAG, SymbolFlags::ERASED),
            (OVERRIDE_TAG, SymbolFlags::OVERRIDE),
        ];

        for (tag, flag) in cases {
            assert_eq!(modifiers(&[tag]).flags, flag, "modifier tag {tag}");
        }
    }

    #[test]
    fn several_modifiers_combine_their_flags() {
        let flags = modifiers(&[FINAL_TAG, CASE_TAG, SEALED_TAG]).flags;

        assert_eq!(
            flags,
            SymbolFlags::FINAL | SymbolFlags::CASE | SymbolFlags::SEALED
        );
    }

    #[test]
    fn object_trait_and_access_modifiers_are_not_flags() {
        let flags =
            modifiers(&[OBJECT_TAG, TRAIT_TAG, PRIVATE_TAG, PROTECTED_TAG, LOCAL_TAG]).flags;

        assert_eq!(flags, SymbolFlags::EMPTY);
    }

    #[test]
    fn a_modifier_without_a_core_flag_is_skipped() {
        use dotty_tasty::tasty::{COVARIANT_TAG, ENUM_TAG, FIELDACCESSOR_TAG};

        for tag in [ENUM_TAG, FIELDACCESSOR_TAG, COVARIANT_TAG] {
            assert_eq!(modifiers(&[tag]), DeclaredModifiers::NONE, "tag {tag}");
        }
    }

    #[test]
    fn object_and_trait_markers_are_recorded() {
        assert!(modifiers(&[OBJECT_TAG]).is_object);
        assert!(modifiers(&[TRAIT_TAG]).is_trait);
        assert!(!modifiers(&[]).is_object);
        assert!(!modifiers(&[]).is_trait);
    }

    // --- kinds ---

    #[test]
    fn a_type_def_with_a_template_is_a_class() {
        assert_eq!(type_def_kind(&modifiers(&[]), true), SymbolKind::Class);
    }

    #[test]
    fn a_type_def_with_a_template_and_trait_is_a_trait() {
        assert_eq!(
            type_def_kind(&modifiers(&[TRAIT_TAG]), true),
            SymbolKind::Trait
        );
    }

    #[test]
    fn a_type_def_with_a_template_and_object_is_a_module_class() {
        assert_eq!(
            type_def_kind(&modifiers(&[OBJECT_TAG]), true),
            SymbolKind::ModuleClass
        );
    }

    #[test]
    fn a_type_def_without_a_template_is_a_type_alias() {
        assert_eq!(type_def_kind(&modifiers(&[]), false), SymbolKind::TypeAlias);
    }

    #[test]
    fn an_opaque_type_def_without_a_template_is_a_type_alias_with_the_opaque_flag() {
        let modifiers = modifiers(&[OPAQUE_TAG]);

        assert_eq!(type_def_kind(&modifiers, false), SymbolKind::TypeAlias);
        assert!(modifiers.flags.contains(SymbolFlags::OPAQUE));
    }

    #[test]
    fn an_object_val_def_is_an_object() {
        assert_eq!(
            val_def_kind(&modifiers(&[OBJECT_TAG]), SymbolKind::Package),
            SymbolKind::Object
        );
    }

    #[test]
    fn a_val_def_in_a_class_is_a_field() {
        assert_eq!(
            val_def_kind(&modifiers(&[]), SymbolKind::Class),
            SymbolKind::Field
        );
    }

    #[test]
    fn a_var_def_in_a_class_is_a_field_marked_mutable() {
        let modifiers = modifiers(&[MUTABLE_TAG]);

        assert_eq!(
            val_def_kind(&modifiers, SymbolKind::Trait),
            SymbolKind::Field
        );
        assert!(modifiers.flags.contains(SymbolFlags::MUTABLE));
    }

    #[test]
    fn a_val_def_in_a_module_class_is_a_field() {
        assert_eq!(
            val_def_kind(&modifiers(&[]), SymbolKind::ModuleClass),
            SymbolKind::Field
        );
    }

    #[test]
    fn a_val_def_in_a_method_is_a_local() {
        assert_eq!(
            val_def_kind(&modifiers(&[]), SymbolKind::Method),
            SymbolKind::Local
        );
    }

    #[test]
    fn the_constructor_name_makes_a_constructor() {
        assert_eq!(def_def_kind("<init>"), SymbolKind::Constructor);
    }

    #[test]
    fn any_other_def_def_name_makes_a_method() {
        assert_eq!(def_def_kind("bar"), SymbolKind::Method);
        assert_eq!(def_def_kind("<init>x"), SymbolKind::Method);
    }

    #[test]
    fn a_public_template_parameter_is_a_field() {
        assert_eq!(term_param_kind(&modifiers(&[]), true), SymbolKind::Field);
    }

    #[test]
    fn a_private_template_parameter_is_still_a_field() {
        assert_eq!(
            term_param_kind(&modifiers(&[PRIVATE_TAG]), true),
            SymbolKind::Field
        );
    }

    #[test]
    fn a_private_this_template_parameter_is_a_plain_parameter() {
        assert_eq!(
            term_param_kind(&modifiers(&[PRIVATE_TAG, LOCAL_TAG]), true),
            SymbolKind::Parameter
        );
    }

    #[test]
    fn a_method_parameter_is_a_parameter_even_when_public() {
        assert_eq!(
            term_param_kind(&modifiers(&[]), false),
            SymbolKind::Parameter
        );
    }

    #[test]
    fn a_type_parameter_is_a_type_parameter() {
        assert_eq!(type_param_kind(), SymbolKind::TypeParameter);
    }

    #[test]
    fn the_real_fixtures_constructor_parameter_and_members_map_as_expected() {
        use crate::names::wire_name;
        use dotty_tasty::tasty::{PACKAGE_TAG, RawTree, StructuredNode, TastyFile};

        let bytes = include_bytes!("../tests/fixtures/semantic/Foo.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let package = file
            .asts()
            .unwrap()
            .iter()
            .find(|node| node.tag == PACKAGE_TAG)
            .unwrap()
            .decode_package()
            .unwrap();
        let StructuredNode::TypeDef(dotty_tasty::tasty::DefinitionBody::TypeDef {
            tail,
            type_or_template: RawTree::LengthNode(template),
            ..
        }) = package
            .stats
            .iter()
            .next()
            .unwrap()
            .decode_structured()
            .unwrap()
        else {
            panic!("Foo is a type definition with a template");
        };
        let template = template.decode_template_structure().unwrap();

        let class = DeclaredModifiers::from_tail(&tail).unwrap();
        let type_param =
            DeclaredModifiers::from_tail(&template.type_params[0].decode_body().unwrap().tail)
                .unwrap();
        let val_param =
            DeclaredModifiers::from_tail(&template.term_params[0].decode_body().unwrap().tail)
                .unwrap();
        let member_names: Vec<_> = template
            .stats
            .iter()
            .map(|stat| {
                let RawTree::LengthNode(node) = stat else {
                    panic!("template stat is a length node");
                };
                let name = node.decode_definition().unwrap().name();
                def_def_kind(&wire_name(file.names(), name).unwrap())
            })
            .collect();

        assert_eq!(type_def_kind(&class, true), SymbolKind::Class);
        assert_eq!(class.visibility, Visibility::Public);
        assert!(type_param.is_private_this());
        assert_eq!(term_param_kind(&val_param, true), SymbolKind::Field);
        assert_eq!(val_param.visibility, Visibility::Public);
        assert_eq!(member_names, [SymbolKind::Constructor, SymbolKind::Method]);
    }
}
