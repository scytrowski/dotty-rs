use crate::source::TextRange;

/// Diagnostic severity reported by a frontend component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Note,
}

/// A source diagnostic with a stable source range and human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    severity: DiagnosticSeverity,
    span: TextRange,
    message: String,
}

impl Diagnostic {
    /// Creates a diagnostic at `span`.
    pub fn new(severity: DiagnosticSeverity, span: TextRange, message: impl Into<String>) -> Self {
        Self {
            severity,
            span,
            message: message.into(),
        }
    }

    /// Creates an error diagnostic.
    pub fn error(span: TextRange, message: impl Into<String>) -> Self {
        Self::new(DiagnosticSeverity::Error, span, message)
    }

    /// Returns the severity.
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    /// Returns the source range associated with the diagnostic.
    pub const fn span(&self) -> TextRange {
        self.span
    }

    /// Returns the diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_an_error_with_its_source_range_and_message() {
        let span = TextRange::new(2, 5).expect("valid range");
        let diagnostic = Diagnostic::error(span, "invalid token");

        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.span(), span);
        assert_eq!(diagnostic.message(), "invalid token");
    }
}
