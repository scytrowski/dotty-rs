use crate::source::TextRange;

/// Diagnostic severity reported by a frontend component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Note,
}

/// A source diagnostic with a stable source range and an issue payload.
///
/// The payload describes the diagnostic; it is not necessarily a formatted
/// message. Use [`Diagnostic<String>`] for the existing human-readable message
/// representation, or another payload type when a frontend needs structured
/// diagnostic data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic<I = String> {
    severity: DiagnosticSeverity,
    span: TextRange,
    issue: I,
}

impl<I> Diagnostic<I> {
    /// Creates a diagnostic with a typed issue payload.
    pub fn with_issue(severity: DiagnosticSeverity, span: TextRange, issue: I) -> Self {
        Self {
            severity,
            span,
            issue,
        }
    }

    /// Returns the diagnostic issue payload.
    pub const fn issue(&self) -> &I {
        &self.issue
    }

    /// Returns the severity.
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    /// Returns the source range associated with the diagnostic.
    pub const fn span(&self) -> TextRange {
        self.span
    }
}

impl Diagnostic<String> {
    /// Creates a diagnostic at `span`.
    pub fn new(severity: DiagnosticSeverity, span: TextRange, message: impl Into<String>) -> Self {
        Self::with_issue(severity, span, message.into())
    }

    /// Creates an error diagnostic.
    pub fn error(span: TextRange, message: impl Into<String>) -> Self {
        Self::new(DiagnosticSeverity::Error, span, message)
    }

    /// Returns the diagnostic message.
    pub fn message(&self) -> &str {
        self.issue()
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
        assert_eq!(diagnostic.issue(), "invalid token");
    }

    #[test]
    fn typed_issue_payload_is_preserved_without_formatting() {
        #[derive(Debug, Clone, PartialEq, Eq)]
        enum IssueKind {
            InvalidToken,
        }

        let span = TextRange::new(2, 5).expect("valid range");
        let diagnostic: Diagnostic<IssueKind> =
            Diagnostic::with_issue(DiagnosticSeverity::Warning, span, IssueKind::InvalidToken);
        let cloned = diagnostic.clone();

        assert_eq!(diagnostic, cloned);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Warning);
        assert_eq!(diagnostic.span(), span);
        assert_eq!(diagnostic.issue(), &IssueKind::InvalidToken);
    }
}
