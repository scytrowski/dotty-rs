//! Presentation of source typing failures from signature and body checking.
use crate::core::{AstArena, DiagnosticSeverity, SourceId, SourceSpan, Untyped};
use crate::typer::TyperError;

/// A typing error reported to a compiler or editor client.
#[derive(Debug, PartialEq, Eq)]
pub struct TypingDiagnostic {
    pub severity: DiagnosticSeverity,
    /// Owned text suitable for reporting after rollback.
    pub message: String,
    /// Source file and UTF-8 byte span when the error can be located.
    pub location: Option<SourceSpan>,
}

impl TypingDiagnostic {
    pub(crate) fn error(message: String, location: Option<SourceSpan>) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            message,
            location,
        }
    }

    /// Reports a typer failure, including after a failed typing attempt has rolled back.
    /// Supply the original untyped arena for the requested source ID. If the source
    /// is unavailable or the error cannot be located by [`crate::typer_error_span_with`],
    /// the message remains available with `location: None`.
    pub fn from_typer<'a>(
        error: &TyperError,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Self {
        let message = match error {
            TyperError::ApplicationArgumentTypeMismatch {
                expected_description,
                actual_description,
                ..
            } => format!("Expected {expected_description}, found {actual_description}"),
            TyperError::ApplicationArityMismatch {
                expected, actual, ..
            } => {
                let noun = if *expected == 1 {
                    "argument"
                } else {
                    "arguments"
                };
                format!("Expected {expected} {noun}, got {actual}")
            }
            _ => error.to_string(),
        };
        Self::error(message, crate::typer_error_span_with(error, ast_for))
    }
}
