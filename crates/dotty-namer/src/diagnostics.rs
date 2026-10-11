//! Structured source diagnostics for naming.
//!
//! These types describe confirmed source issues independently from
//! [`crate::NamerError`], which also represents unsupported AST shapes and
//! internal invariants. In particular, a `NamerError::InvalidVisibilityQualifier`
//! is not automatically a [`NamerIssue`]: owner resolution must first establish
//! that the qualifier is invalid for the source declaration.

use dotty_core::{Diagnostic, DiagnosticSeverity, SourceId, SourceSpan, TextRange};

/// Visibility modifier associated with an invalid qualified access boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibilityAccess {
    /// The source used `private[qualifier]`.
    Private,
    /// The source used `protected[qualifier]`.
    Protected,
}

/// A confirmed issue in the user's Scala source encountered while naming.
///
/// This contains only source-level issues. Structural `NamerError`s and
/// unsupported-but-valid constructs do not belong here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamerIssue {
    /// A qualified visibility modifier names no valid enclosing access boundary.
    ///
    /// `qualifier` owns its spelling so the issue remains inspectable after
    /// rollback or after the source name interner is dropped. This variant
    /// should only be constructed once naming has ruled out an owner-resolution
    /// gap and parser-recovery effects; it is not a direct mapping from every
    /// `NamerError::InvalidVisibilityQualifier`.
    InvalidVisibilityQualifier {
        /// Whether the source modifier was private or protected.
        access: VisibilityAccess,
        /// Source spelling of the unresolved qualifier.
        qualifier: String,
    },
}

impl NamerIssue {
    /// Returns the stable machine-readable code for this issue.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidVisibilityQualifier { .. } => "namer.visibility.invalid_qualifier",
        }
    }
}

/// A typed naming diagnostic that retains the source file identity.
///
/// [`Diagnostic`] stores a byte range but not a [`SourceId`]. This wrapper
/// carries both without duplicating the range, following the parser's
/// `ParseDiagnostic` shape. Naming issues currently represent errors, so the
/// constructor assigns [`DiagnosticSeverity::Error`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamerDiagnostic {
    source: SourceId,
    diagnostic: Diagnostic<NamerIssue>,
}

impl NamerDiagnostic {
    /// Creates a source error diagnostic from a real source span and typed issue.
    pub fn error(span: SourceSpan, issue: NamerIssue) -> Self {
        Self {
            source: span.source(),
            diagnostic: Diagnostic::with_issue(
                DiagnosticSeverity::Error,
                span.span().range(),
                issue,
            ),
        }
    }

    /// Returns the structured naming issue.
    pub const fn issue(&self) -> &NamerIssue {
        self.diagnostic.issue()
    }

    /// Returns the issue's stable machine-readable code.
    pub const fn code(&self) -> &'static str {
        self.diagnostic.issue().code()
    }

    /// Returns the diagnostic severity.
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.diagnostic.severity()
    }

    /// Returns the source file identity.
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// Returns the source byte range of the diagnostic.
    pub const fn span(&self) -> TextRange {
        self.diagnostic.span()
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::{SemanticStore, SourceText, Span, TextRange};

    use super::*;

    #[test]
    fn visibility_issue_keeps_source_identity_utf8_range_severity_and_code() {
        let source_text = SourceText::new("private[École]").unwrap();
        let range = TextRange::new(8, 14).unwrap();
        assert_eq!(source_text.slice(range), Ok("École"));
        let source = SourceId::from_index(42);
        let source_span = SourceSpan::new(source, Span::without_point(range));
        let diagnostic = NamerDiagnostic::error(
            source_span,
            NamerIssue::InvalidVisibilityQualifier {
                access: VisibilityAccess::Private,
                qualifier: "École".to_owned(),
            },
        );

        assert_eq!(diagnostic.source(), source);
        assert_eq!(diagnostic.span(), range);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.code(), "namer.visibility.invalid_qualifier");
        assert_eq!(
            diagnostic.issue(),
            &NamerIssue::InvalidVisibilityQualifier {
                access: VisibilityAccess::Private,
                qualifier: "École".to_owned(),
            }
        );
    }

    #[test]
    fn protected_visibility_uses_the_same_stable_issue_code() {
        let range = TextRange::new(0, 1).unwrap();
        let diagnostic = NamerDiagnostic::error(
            SourceSpan::new(SourceId::from_index(43), Span::without_point(range)),
            NamerIssue::InvalidVisibilityQualifier {
                access: VisibilityAccess::Protected,
                qualifier: "Outer".to_owned(),
            },
        );

        assert_eq!(diagnostic.code(), "namer.visibility.invalid_qualifier");
        assert!(matches!(
            diagnostic.issue(),
            NamerIssue::InvalidVisibilityQualifier {
                access: VisibilityAccess::Protected,
                qualifier,
            } if qualifier == "Outer"
        ));
    }

    #[test]
    fn issue_payload_owns_qualifier_spelling_after_interner_is_dropped() {
        let mut store = SemanticStore::new();
        let qualifier = store.names.intern("Missing");
        let spelling = store.names.resolve(qualifier).to_owned();
        let diagnostic = NamerDiagnostic::error(
            SourceSpan::new(
                SourceId::from_index(44),
                Span::without_point(TextRange::new(0, 7).unwrap()),
            ),
            NamerIssue::InvalidVisibilityQualifier {
                access: VisibilityAccess::Private,
                qualifier: spelling,
            },
        );
        drop(store);

        assert!(matches!(
            diagnostic.issue(),
            NamerIssue::InvalidVisibilityQualifier { qualifier, .. } if qualifier == "Missing"
        ));
    }

    #[test]
    fn naming_diagnostic_clones_and_compares_structurally() {
        let diagnostic = NamerDiagnostic::error(
            SourceSpan::new(
                SourceId::from_index(45),
                Span::without_point(TextRange::new(3, 8).unwrap()),
            ),
            NamerIssue::InvalidVisibilityQualifier {
                access: VisibilityAccess::Private,
                qualifier: "Outer".to_owned(),
            },
        );

        assert_eq!(diagnostic, diagnostic.clone());
    }
}
