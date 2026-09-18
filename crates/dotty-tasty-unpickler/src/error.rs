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
            Self::DuplicateDefinition { .. } | Self::DuplicateScope { .. } => None,
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
