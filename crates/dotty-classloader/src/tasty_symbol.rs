use crate::binary_name::BinaryName;
use dotty_classfile::access_flags::{
    ACC_ABSTRACT, ACC_FINAL, ACC_INTERFACE, ACC_PUBLIC, ClassAccessFlags,
};
use dotty_tasty::tasty::{
    ABSTRACT_TAG, AstError, DefinitionBody, DefinitionTail, FINAL_TAG, PRIVATE_TAG, PROTECTED_TAG,
    RawName, RawTree, StructuredNode, TEMPLATE_TAG, TRAIT_TAG, TYPEDEF_TAG, TastyFile,
    TastyFileError,
};
use std::fmt;

/// The class-level facts reconstructable from a `.tasty` file without
/// full type-checking: enough to build a `ClassSymbol` shell and recurse
/// into its supertypes, mirroring what `.class` decoding gives
/// (`docs/classloader.md` §9). `fields`/`methods`/`signature`/
/// `nest_host`/etc. are not reconstructed from `.tasty` yet — the
/// caller passes empty/`None` for those, the same "not yet populated"
/// shape earlier `.class` milestones used before their corresponding
/// feature landed.
#[derive(Debug)]
pub(crate) struct DecodedTastyClass {
    pub flags: ClassAccessFlags,
    pub super_class: BinaryName,
    pub interfaces: Vec<BinaryName>,
}

/// Why decoding a `.tasty` file into a [`DecodedTastyClass`] failed.
///
/// `pub`, not `pub(crate)`, even though the owning `tasty_symbol` module
/// is private: this type appears as a field of the public
/// [`crate::error::ClassLoadError::InvalidTastyFile`] variant and is
/// re-exported through the `classloader` facade module so it stays
/// nameable from outside the crate, the same treatment every other
/// symbol/error type introduced by a milestone gets.
#[derive(Debug, Clone)]
pub enum TastyDecodeError {
    /// The bytes did not parse as a Scala 3.9 `.tasty` file at all.
    Parse(TastyFileError),
    /// The file parsed, but indexing/decoding its AST nodes failed.
    Ast(AstError),
    /// No top-level `TypeDef` in the file has the requested class's
    /// simple name (a `.tasty` file can define several `TypeDef`s for
    /// one source class — the class itself, its companion module, a
    /// `Mirror`-derived synthetic type — so this is not "the file is
    /// empty", just "none of them match").
    MissingTypeDef,
    /// The matching `TypeDef` has no `Template` body to read parents
    /// from (there is always one for a class/trait/object; seeing this
    /// means the file's shape isn't one this decoder recognizes).
    NoTemplateBody,
    /// A supertype after the first `Template` parent (i.e. a mixin
    /// interface, not the implicit superclass slot) has no name this
    /// decoder can resolve — see [`resolve_parent_name`]'s doc comment
    /// for exactly which shapes are and are not resolved.
    UnresolvedSupertype,
}

impl fmt::Display for TastyDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(source) => write!(formatter, "failed to parse .tasty file: {source}"),
            Self::Ast(source) => write!(formatter, "failed to decode .tasty AST: {source}"),
            Self::MissingTypeDef => {
                write!(formatter, "no matching TypeDef found in .tasty file")
            }
            Self::NoTemplateBody => write!(formatter, "TypeDef has no Template body"),
            Self::UnresolvedSupertype => {
                write!(
                    formatter,
                    "a supertype reference could not be resolved to a name"
                )
            }
        }
    }
}

impl std::error::Error for TastyDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(source) => Some(source),
            Self::Ast(source) => Some(source),
            Self::MissingTypeDef | Self::NoTemplateBody | Self::UnresolvedSupertype => None,
        }
    }
}

impl From<TastyFileError> for TastyDecodeError {
    fn from(source: TastyFileError) -> Self {
        Self::Parse(source)
    }
}

impl From<AstError> for TastyDecodeError {
    fn from(source: AstError) -> Self {
        Self::Ast(source)
    }
}

/// Decodes `bytes` as a `.tasty` file and reconstructs the class named
/// `requested` (matched by its simple name — the part after the last
/// `/` — since a `.tasty` file's own `TypeDef`s carry no package
/// qualification of their own).
pub(crate) fn decode(
    bytes: &[u8],
    requested: &BinaryName,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    let file = TastyFile::parse_scala_3_9(bytes)?;
    let index = file.ast_address_index()?;
    let simple_name = requested
        .as_internal()
        .rsplit('/')
        .next()
        .unwrap_or(requested.as_internal());

    let node = index
        .iter()
        .find(|node| {
            if node.tag != TYPEDEF_TAG {
                return false;
            }
            let Ok(definition) = node.decode_definition() else {
                return false;
            };
            wire_name(&file, definition.name()).as_deref() == Some(simple_name)
        })
        .ok_or(TastyDecodeError::MissingTypeDef)?;

    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
        tail,
        type_or_template,
        ..
    }) = node.decode_structured()?
    else {
        return Err(TastyDecodeError::MissingTypeDef);
    };

    let flags = decode_flags(&tail);

    let RawTree::LengthNode(template_node) = &type_or_template else {
        return Err(TastyDecodeError::NoTemplateBody);
    };
    if template_node.tag != TEMPLATE_TAG {
        return Err(TastyDecodeError::NoTemplateBody);
    }
    let StructuredNode::Template(template) = template_node.decode_structured()? else {
        return Err(TastyDecodeError::NoTemplateBody);
    };

    let mut parents = template.parents.iter();
    let super_class = match parents.next() {
        Some(parent) => resolve_parent_name(&file, parent)
            .unwrap_or_else(|| BinaryName::from_internal("java/lang/Object")),
        None => BinaryName::from_internal("java/lang/Object"),
    };

    let mut interfaces = Vec::new();
    for parent in parents {
        interfaces
            .push(resolve_parent_name(&file, parent).ok_or(TastyDecodeError::UnresolvedSupertype)?);
    }

    Ok(DecodedTastyClass {
        flags,
        super_class,
        interfaces,
    })
}

fn decode_flags(tail: &[DefinitionTail<'_>]) -> ClassAccessFlags {
    let mut bits = ACC_PUBLIC;

    for item in tail {
        let DefinitionTail::Modifier(tag) = item else {
            continue;
        };

        match *tag {
            TRAIT_TAG => bits |= ACC_INTERFACE | ACC_ABSTRACT,
            ABSTRACT_TAG => bits |= ACC_ABSTRACT,
            FINAL_TAG => bits |= ACC_FINAL,
            PRIVATE_TAG | PROTECTED_TAG => bits &= !ACC_PUBLIC,
            _ => {}
        }
    }

    ClassAccessFlags(bits)
}

/// Reconstructs a supertype parent's referenced [`BinaryName`], or
/// `None` when it can't be — either because the parent carries no name
/// reference at all (the implicit `Object`/`AnyRef` superclass
/// constructor call, which `.tasty` encodes without any name-table
/// entry, JVMS §4.1's implicit interface superclass follows the same
/// shape), or because every name reference it does carry fails to
/// render as a plain name.
///
/// [`RawTree::name_refs`] returns every name-table reference visible in
/// a parent tree in source order; empirically (see this crate's
/// Milestone 9 plan), a same-package mixin (`class Dog extends Animal`)
/// carries exactly one directly-named reference (`"Animal"`) followed by
/// an unrelated symbol reference with no direct UTF-8 name, while a
/// cross-package mixin (`case class Point`'s implicit `Product`/
/// `Serializable`) carries a fully-named, reversed qualification chain
/// (e.g. `["Product", "scala", "_root_"]`). Filtering to only the
/// references that do resolve to a direct name, in order, handles both
/// shapes uniformly: reverse them, drop a leading synthetic `_root_`
/// root-package marker, and join with `/`.
fn resolve_parent_name(file: &TastyFile<'_>, parent: &RawTree<'_>) -> Option<BinaryName> {
    let mut names: Vec<String> = parent
        .name_refs()
        .into_iter()
        .filter_map(|reference| wire_name(file, reference))
        .collect();

    if names.is_empty() {
        return None;
    }

    names.reverse();
    if names.first().map(String::as_str) == Some("_root_") {
        names.remove(0);
    }
    if names.is_empty() {
        return None;
    }

    Some(BinaryName::from_internal(names.join("/")))
}

/// Reads a raw AST name-table reference directly rather than through
/// [`TastyFile::render_name`]: AST fields such as [`RawTree::name_refs`]
/// and a definition's own `name()` are zero-based direct indices into
/// the wire name table, not the one-based [`dotty_tasty::tasty::NameRef`]
/// convention `render_name`/`NameTable::get` expect (matching how
/// `dotty-tasty`'s own fixture tests read these same fields). Returns
/// `None` for anything other than a direct UTF-8 entry (a signature- or
/// symbol-shaped name-table entry, which this decoder does not need to
/// render).
fn wire_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    file.names()
        .entries()
        .get(reference as usize)
        .and_then(RawName::as_utf8)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bytes(relative_path: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty/tests/fixtures")
                .join(relative_path),
        )
        .unwrap_or_else(|error| panic!("fixture {relative_path} should exist: {error}"))
    }

    #[test]
    fn decodes_a_trait_as_an_interface_with_no_declared_interfaces() {
        let decoded = decode(
            &fixture_bytes("inheritance/Animal.tasty"),
            &BinaryName::from_internal("Animal"),
        )
        .unwrap();

        assert!(decoded.flags.is_interface());
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert!(decoded.interfaces.is_empty());
    }

    #[test]
    fn decodes_a_class_with_a_trait_mixin_as_an_interface() {
        let decoded = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("Dog"),
        )
        .unwrap();

        assert!(!decoded.flags.is_interface());
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal("Animal")]
        );
    }

    #[test]
    fn decodes_a_case_class_with_cross_package_mixins() {
        let decoded = decode(
            &fixture_bytes("case_class/Point.tasty"),
            &BinaryName::from_internal("Point"),
        )
        .unwrap();

        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![
                BinaryName::from_internal("scala/Product"),
                BinaryName::from_internal("scala/Serializable"),
            ]
        );
    }

    #[test]
    fn missing_type_def_when_no_definition_matches_the_requested_name() {
        let error = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("DoesNotExist"),
        )
        .unwrap_err();

        assert!(matches!(error, TastyDecodeError::MissingTypeDef));
    }

    #[test]
    fn parse_error_when_the_bytes_are_not_a_tasty_file() {
        let error =
            decode(b"not a tasty file", &BinaryName::from_internal("Anything")).unwrap_err();

        assert!(matches!(error, TastyDecodeError::Parse(_)));
    }

    /// `decode_flags` is a pure, narrow mapping function (the tag
    /// values below are real ones — confirmed against `dotty-tasty`'s
    /// own `ast.rs` constants and cross-checked by scanning every
    /// committed real fixture's `TypeDef` modifiers for each tag), so
    /// its modifier-to-`ClassAccessFlags` mapping is tested directly
    /// against hand-built `tail` vectors, the same treatment
    /// `manifest.rs`'s hand-written-input tests give its own narrow
    /// parsing concern. Full end-to-end `decode()` coverage of a real
    /// `abstract`/`final`/`private` class is a separate concern — the
    /// three real `scala3-library` fixtures confirmed to carry these
    /// modifiers (`scala/io/Source.tasty`'s `Source`, `scala/math/
    /// BigInt.tasty`'s `BigInt`, `scala/math/Ordering.tasty`'s private
    /// `Reverse`) all currently fail `decode()` with `UnresolvedSupertype`
    /// on a later parent, which is a real, disclosed limitation, not a
    /// missing test.
    #[test]
    fn decode_flags_marks_trait_as_interface_and_abstract() {
        let flags = decode_flags(&[DefinitionTail::Modifier(TRAIT_TAG)]);

        assert!(flags.is_interface());
        assert!(flags.is_abstract());
    }

    #[test]
    fn decode_flags_marks_abstract_classes() {
        let flags = decode_flags(&[DefinitionTail::Modifier(ABSTRACT_TAG)]);

        assert!(flags.is_abstract());
        assert!(!flags.is_interface());
    }

    #[test]
    fn decode_flags_marks_final_classes() {
        let flags = decode_flags(&[DefinitionTail::Modifier(FINAL_TAG)]);

        assert!(flags.is_final());
    }

    #[test]
    fn decode_flags_clears_public_for_a_private_modifier() {
        let flags = decode_flags(&[DefinitionTail::Modifier(PRIVATE_TAG)]);

        assert!(!flags.is_public());
    }

    #[test]
    fn decode_flags_clears_public_for_a_protected_modifier() {
        let flags = decode_flags(&[DefinitionTail::Modifier(PROTECTED_TAG)]);

        assert!(!flags.is_public());
    }

    #[test]
    fn decode_flags_defaults_to_public_with_no_modifiers() {
        let flags = decode_flags(&[]);

        assert!(flags.is_public());
    }
}
