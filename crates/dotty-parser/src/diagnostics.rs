use dotty_core::{Diagnostic, DiagnosticSeverity, SourceId, SourceSpan, TextRange, TokenKind};

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

/// Structured parser issue data.
///
/// `Legacy` temporarily retains messages from parser call sites that have not
/// yet migrated to typed issues. Typed variants carry issue data rather than
/// preformatted diagnostic text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseIssue {
    /// Transitional payload for parser diagnostics that still use free-form
    /// messages. New reporting code should use a typed variant instead.
    Legacy {
        kind: ParseDiagnosticKind,
        message: String,
    },
    /// A required parser-facing token was absent.
    ExpectedToken {
        expected: TokenKind,
        found: TokenKind,
    },
    /// An expression was required at the current token.
    ExpectedExpression { found: TokenKind },
    /// A type was required at the current token.
    ExpectedType { found: TokenKind },
    /// A pattern was required at the current token.
    ExpectedPattern { found: TokenKind },
    /// A token is not valid in the current parser position.
    UnexpectedToken { found: TokenKind },
    /// The expression fragment contains tokens after its expression.
    TrailingInput { found: TokenKind },
    /// A placeholder parameter escaped the expression that owns it.
    UnboundPlaceholderParameter,
}

impl ParseIssue {
    /// Returns the stable parser diagnostic category for this issue.
    pub const fn kind(&self) -> ParseDiagnosticKind {
        match self {
            Self::Legacy { kind, .. } => *kind,
            Self::ExpectedToken { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::ExpectedExpression { .. } => ParseDiagnosticKind::ExpectedExpression,
            Self::ExpectedType { .. } => ParseDiagnosticKind::ExpectedType,
            Self::ExpectedPattern { .. } => ParseDiagnosticKind::ExpectedPattern,
            Self::UnexpectedToken { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::TrailingInput { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::UnboundPlaceholderParameter => ParseDiagnosticKind::UnboundPlaceholderParameter,
        }
    }

    /// Returns a stable, text-independent identifier for corpus reporting.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Legacy { kind, .. } => match kind {
                ParseDiagnosticKind::ExpectedToken => "parser.expected_token",
                ParseDiagnosticKind::UnexpectedToken => "parser.unexpected_token",
                ParseDiagnosticKind::ExpectedExpression => "parser.expected_expression",
                ParseDiagnosticKind::ExpectedType => "parser.expected_type",
                ParseDiagnosticKind::ExpectedPattern => "parser.expected_pattern",
                ParseDiagnosticKind::UnsupportedSyntax => "parser.unsupported_syntax",
                ParseDiagnosticKind::UnboundPlaceholderParameter => {
                    "parser.unbound_placeholder_parameter"
                }
            },
            Self::ExpectedToken { .. } => "parser.expected_token",
            Self::ExpectedExpression { .. } => "parser.expected_expression",
            Self::ExpectedType { .. } => "parser.expected_type",
            Self::ExpectedPattern { .. } => "parser.expected_pattern",
            Self::UnexpectedToken { .. } => "parser.unexpected_token",
            Self::TrailingInput { .. } => "parser.trailing_input",
            Self::UnboundPlaceholderParameter => "parser.unbound_placeholder_parameter",
        }
    }
}

/// A parser diagnostic that retains the shared diagnostic payload and source identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseDiagnostic {
    source: SourceId,
    diagnostic: Diagnostic<ParseIssue>,
}

impl ParseDiagnostic {
    /// Creates a transitional legacy-message error diagnostic at `span`.
    pub fn error(kind: ParseDiagnosticKind, span: SourceSpan, message: impl Into<String>) -> Self {
        Self::with_issue(
            span,
            ParseIssue::Legacy {
                kind,
                message: message.into(),
            },
        )
    }

    /// Creates a parser error diagnostic with structured issue data.
    pub fn with_issue(span: SourceSpan, issue: ParseIssue) -> Self {
        Self {
            source: span.source(),
            diagnostic: Diagnostic::with_issue(
                DiagnosticSeverity::Error,
                span.span().range(),
                issue,
            ),
        }
    }

    /// Returns the structured parser issue payload.
    pub fn issue(&self) -> &ParseIssue {
        self.diagnostic.issue()
    }

    /// Returns the parser-specific diagnostic category.
    ///
    /// This compatibility classification is derived from the issue payload;
    /// typed callers should inspect [`Self::issue`] for its structured data.
    pub const fn kind(&self) -> ParseDiagnosticKind {
        self.diagnostic.issue().kind()
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

    /// Returns the message when this issue still uses the transitional legacy
    /// payload. Typed issues intentionally have no rendered message here.
    pub fn legacy_message(&self) -> Option<&str> {
        match self.issue() {
            ParseIssue::Legacy { message, .. } => Some(message),
            ParseIssue::ExpectedToken { .. }
            | ParseIssue::ExpectedExpression { .. }
            | ParseIssue::ExpectedType { .. }
            | ParseIssue::ExpectedPattern { .. }
            | ParseIssue::UnexpectedToken { .. }
            | ParseIssue::TrailingInput { .. }
            | ParseIssue::UnboundPlaceholderParameter => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Span, TextRange};

    #[test]
    fn parser_diagnostic_preserves_legacy_kind_source_range_and_message() {
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
        assert_eq!(diagnostic.legacy_message(), Some("expected expression"));
        assert!(matches!(diagnostic.issue(), ParseIssue::Legacy { .. }));
    }

    #[test]
    fn typed_issue_preserves_source_severity_payload_and_clone_without_message() {
        let range = TextRange::new(2, 5).expect("valid range");
        let source = SourceId::from_index(7);
        let diagnostic = ParseDiagnostic::with_issue(
            SourceSpan::new(source, Span::without_point(range)),
            ParseIssue::ExpectedToken {
                expected: TokenKind::Keyword(dotty_core::HardKeyword::If),
                found: TokenKind::Identifier,
            },
        );
        let cloned = diagnostic.clone();

        assert_eq!(diagnostic, cloned);
        assert_eq!(diagnostic.kind(), ParseDiagnosticKind::ExpectedToken);
        assert_eq!(diagnostic.source(), source);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.span(), range);
        assert!(matches!(
            diagnostic.issue(),
            ParseIssue::ExpectedToken {
                expected: TokenKind::Keyword(dotty_core::HardKeyword::If),
                found: TokenKind::Identifier
            }
        ));
        assert_eq!(diagnostic.legacy_message(), None);
    }
}
