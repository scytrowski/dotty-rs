use dotty_core::{Diagnostic, DiagnosticSeverity, SourceId, SourceSpan, TextRange};

/// Parser-specific category for a recoverable diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseDiagnosticKind {
    ExpectedToken,
    UnexpectedToken,
    ExpectedExpression,
    ExpectedType,
    ExpectedPattern,
    UnsupportedSyntax,
    UnboundPlaceholderParameter,
}

/// A parser diagnostic that retains the shared diagnostic payload and source identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseDiagnostic {
    kind: ParseDiagnosticKind,
    source: SourceId,
    diagnostic: Diagnostic,
}

impl ParseDiagnostic {
    /// Creates an error diagnostic at a source span.
    pub fn error(kind: ParseDiagnosticKind, span: SourceSpan, message: impl Into<String>) -> Self {
        Self {
            kind,
            source: span.source(),
            diagnostic: Diagnostic::error(span.span().range(), message),
        }
    }

    /// Returns the parser-specific diagnostic category.
    pub const fn kind(&self) -> ParseDiagnosticKind {
        self.kind
    }

    /// Returns the source file identity.
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// Returns the shared diagnostic severity.
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.diagnostic.severity()
    }

    /// Returns the source range of the diagnostic.
    pub const fn span(&self) -> TextRange {
        self.diagnostic.span()
    }

    /// Returns the human-readable diagnostic message.
    pub fn message(&self) -> &str {
        self.diagnostic.message()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Span, TextRange};

    #[test]
    fn parser_diagnostic_preserves_kind_source_and_range() {
        let range = TextRange::new(2, 5).expect("valid range");
        let source = SourceId::from_index(4);
        let diagnostic = ParseDiagnostic::error(
            ParseDiagnosticKind::ExpectedExpression,
            SourceSpan::new(source, Span::without_point(range)),
            "expected expression",
        );

        assert_eq!(diagnostic.kind(), ParseDiagnosticKind::ExpectedExpression);
        assert_eq!(diagnostic.source(), source);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.span(), range);
        assert_eq!(diagnostic.message(), "expected expression");
    }
}
