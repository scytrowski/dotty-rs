use std::fmt;

use dotty_core::ids::SymbolId;
use dotty_tasty::tasty::{AstError, TastyFileError};

/// Why semantic unpickling of a TASTy file failed.
///
/// Malformed or unsupported TASTy input is always reported through this type;
/// the unpickler never lowers an unsupported semantic shape to a placeholder.
/// Variants are introduced only when an unpickling pass needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpickleError {
    /// The TASTy file itself failed a structural query, such as building the
    /// AST address index.
    Tasty(TastyFileError),
    /// An AST node could not be decoded into its structured form.
    Ast(AstError),
    /// A second symbol was entered for a definition address that already has
    /// one, which would break the address-identity invariant.
    DuplicateDefinition { address: u32 },
    /// A second declaration scope was entered for a symbol that already owns
    /// one.
    DuplicateScope { symbol: SymbolId },
    /// A name reference does not resolve to a name-table entry, or resolves
    /// through an unreasonably deep (possibly cyclic) chain of entries.
    InvalidNameReference { reference: u32 },
    /// A name-table entry with a tag this unpickler does not interpret.
    UnsupportedName { reference: u32 },
    /// The qualifier of a `private[X]` / `protected[X]` modifier has a tree
    /// shape this unpickler does not read. `tag` is the qualifier node's tag.
    /// Package names and references to enclosing definitions are supported.
    UnsupportedQualifier { tag: u8 },
    /// A reference points at an address that is not the start of a visible AST
    /// node (out of range, or inside another node's payload), or at one that
    /// has no entered symbol. `from` is the referring definition.
    InvalidReferenceTarget { from: u32, to: u32 },
    /// The qualifier of a `private[Q]` / `protected[Q]` modifier at
    /// `definition` is not a package, class, trait or object that encloses
    /// the qualified definition (or is the definition itself).
    InvalidQualifier { definition: u32 },
    /// A `PACKAGE` node whose path is neither a direct package reference
    /// (`TERMREFpkg`) nor a `SHAREDtype` link to one, which are the only forms
    /// this unpickler reads.
    UnsupportedPackagePath { address: u32 },
    /// No definition node exists at an address the tree walk expected one.
    MissingDefinition { address: u32 },
    /// A second `TypeId` was recorded for a type-node address that already
    /// has one, which would break the address-identity invariant.
    DuplicateType { address: u32 },
    /// The type node at `address` has a tag the type pass does not decode
    /// yet (a later increment) or that is not a type at all. The node is
    /// never lowered to a placeholder type.
    UnsupportedType { tag: u8, address: u32 },
    /// The type node at `from` refers to the definition at `to`, which is a
    /// visible AST node but has no symbol entered by pass 1 (for example a
    /// local definition, or a definition in another unit).
    MissingReferencedSymbol { from: u32, to: u32 },
    /// The type node at `from` refers to the definition at `to`, which has an
    /// entered symbol of the wrong kind: a `TYPEREF*` must name a type-namespace
    /// symbol, a `TERMREF*` a term-namespace one, and `THIS` a class.
    InvalidReferenceKind { from: u32, to: u32 },
    /// The `TYPEREFpkg` / `TERMREFpkg` node at `address` names a package
    /// that has not been entered into the package registry. Resolving
    /// packages outside the entered units belongs to the future resolver.
    UnresolvedPackage { address: u32, package: String },
}

impl fmt::Display for UnpickleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tasty(error) => write!(formatter, "invalid TASTy file: {error}"),
            Self::Ast(error) => write!(formatter, "invalid TASTy AST node: {error}"),
            Self::DuplicateDefinition { address } => write!(
                formatter,
                "a symbol was already entered for the definition at address {address}"
            ),
            Self::InvalidNameReference { reference } => {
                write!(formatter, "invalid name reference {reference}")
            }
            Self::UnsupportedName { reference } => {
                write!(formatter, "unsupported name entry at reference {reference}")
            }
            Self::UnsupportedQualifier { tag } => write!(
                formatter,
                "access qualifier with tag {tag} is not a package or enclosing definition"
            ),
            Self::InvalidReferenceTarget { from, to } => write!(
                formatter,
                "definition at address {from} refers to address {to}, which is not a valid target"
            ),
            Self::InvalidQualifier { definition } => write!(
                formatter,
                "the access qualifier of the definition at address {definition} does not enclose it"
            ),
            Self::UnsupportedPackagePath { address } => write!(
                formatter,
                "package at address {address} has an unsupported path form"
            ),
            Self::MissingDefinition { address } => {
                write!(formatter, "no definition node at address {address}")
            }
            Self::DuplicateType { address } => write!(
                formatter,
                "a type was already recorded for the type node at address {address}"
            ),
            Self::UnsupportedType { tag, address } => write!(
                formatter,
                "the type node at address {address} has tag {tag}, which is not decoded yet"
            ),
            Self::MissingReferencedSymbol { from, to } => write!(
                formatter,
                "the type node at address {from} refers to address {to}, which has no entered symbol"
            ),
            Self::InvalidReferenceKind { from, to } => write!(
                formatter,
                "the type node at address {from} refers to the definition at address {to}, which is the wrong kind of symbol for it"
            ),
            Self::UnresolvedPackage { address, package } => write!(
                formatter,
                "the package reference at address {address} names package `{package}`, which has not been entered"
            ),
            Self::DuplicateScope { symbol } => write!(
                formatter,
                "a declaration scope was already entered for symbol {}",
                symbol.index()
            ),
        }
    }
}

impl std::error::Error for UnpickleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tasty(error) => Some(error),
            Self::Ast(error) => Some(error),
            Self::DuplicateDefinition { .. }
            | Self::DuplicateScope { .. }
            | Self::InvalidNameReference { .. }
            | Self::UnsupportedName { .. }
            | Self::UnsupportedQualifier { .. }
            | Self::InvalidReferenceTarget { .. }
            | Self::InvalidQualifier { .. }
            | Self::UnsupportedPackagePath { .. }
            | Self::MissingDefinition { .. }
            | Self::DuplicateType { .. }
            | Self::UnsupportedType { .. }
            | Self::MissingReferencedSymbol { .. }
            | Self::InvalidReferenceKind { .. }
            | Self::UnresolvedPackage { .. } => None,
        }
    }
}

impl From<TastyFileError> for UnpickleError {
    fn from(error: TastyFileError) -> Self {
        Self::Tasty(error)
    }
}

impl From<AstError> for UnpickleError {
    fn from(error: AstError) -> Self {
        Self::Ast(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::HeaderError;

    #[test]
    fn tasty_file_errors_convert_and_expose_their_source() {
        let source = TastyFileError::Header(HeaderError::InvalidMagic {
            actual: [0, 0, 0, 0],
        });
        let error = UnpickleError::from(source.clone());

        assert_eq!(error, UnpickleError::Tasty(source));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("invalid TASTy file"));
    }

    #[test]
    fn duplicate_definition_names_the_address_and_has_no_source() {
        let error = UnpickleError::DuplicateDefinition { address: 46 };

        assert!(error.to_string().contains("address 46"));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn ast_errors_convert_and_expose_their_source() {
        let source = AstError::InvalidTag { tag: 1, offset: 7 };
        let error = UnpickleError::from(source.clone());

        assert_eq!(error, UnpickleError::Ast(source));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("invalid TASTy AST node"));
    }
}
