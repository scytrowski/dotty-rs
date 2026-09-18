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
    /// A `private[X]` or `protected[X]` modifier. `dotty-core`'s `Visibility`
    /// has no qualified-access variant yet, so it cannot be represented
    /// faithfully.
    UnsupportedQualifiedModifier { tag: u8 },
    /// A `PACKAGE` node whose path is not a direct package reference
    /// (`TERMREFpkg`), which is the only form this unpickler reads.
    UnsupportedPackagePath { address: u32 },
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
            Self::UnsupportedQualifiedModifier { tag } => write!(
                formatter,
                "qualified access modifier (tag {tag}) has no visibility representation"
            ),
            Self::UnsupportedPackagePath { address } => write!(
                formatter,
                "package at address {address} has an unsupported path form"
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
            | Self::UnsupportedQualifiedModifier { .. }
            | Self::UnsupportedPackagePath { .. } => None,
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
