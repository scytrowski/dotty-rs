//! Preparing an individual bound source for ordinary-class analysis.
use crate::core::{SourceId, SourceSpan, Span};
use crate::namer::{NamerDiagnostic, NamerError};
use crate::source_declarations::{DeclarationDiagnostic, DeclarationError};
use crate::{SourceDiagnostic, SyntaxDiagnostic};

/// An owned finding from syntax, naming or ordinary-class projection. Clients
/// can inspect the original payload without rendering it. Byte ranges refer
/// to the original document version and IDs belong to the original session.
#[derive(Debug)]
pub enum PreparationDiagnostic {
    /// A lexer or parser issue from the bound parse.
    Syntax(SyntaxDiagnostic),
    /// A confirmed source issue from the namer.
    Naming(NamerDiagnostic),
    /// Naming could not finish because of a structural failure.
    NamerFailure {
        /// Source of the attempted naming transaction.
        source: SourceId,
        /// Original namer error, including variants without a source location.
        error: NamerError,
    },
    /// A declaration outside the projected subset, or an invalid binding.
    Declaration(DeclarationDiagnostic),
}

impl PreparationDiagnostic {
    /// Original file even when a diagnostic has no source range.
    pub fn source(&self) -> SourceId {
        match self {
            Self::Syntax(d) => d.location().source(),
            Self::Naming(d) => d.source(),
            Self::NamerFailure { source, .. } => *source,
            Self::Declaration(d) => d.error.source_id(),
        }
    }

    /// Original UTF-8 byte span, if supplied by the phase.
    pub fn location(&self) -> Option<SourceSpan> {
        match self {
            Self::Syntax(d) => Some(d.location()),
            Self::Naming(d) => Some(SourceSpan::new(d.source(), Span::without_point(d.span()))),
            Self::NamerFailure { .. } => None,
            Self::Declaration(d) => d.location,
        }
    }

    /// Stable machine-readable code. Structural naming failures have no code.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::Syntax(SyntaxDiagnostic::Lexer { diagnostic, .. }) => diagnostic.issue().code(),
            Self::Syntax(SyntaxDiagnostic::Parser(d)) => d.issue().code(),
            Self::Naming(d) => d.code(),
            Self::NamerFailure { .. } => return None,
            Self::Declaration(d) => match d.error {
                DeclarationError::Unsupported { .. } => "declaration.unsupported",
                DeclarationError::InvalidBinding { .. } => "declaration.invalid_binding",
            },
        })
    }

    /// Renders the default message at publication time without consulting the session.
    pub fn render(&self) -> SourceDiagnostic {
        match self {
            Self::Syntax(d) => d.render(),
            Self::Naming(d) => SourceDiagnostic::from_namer(d),
            Self::NamerFailure { error, .. } => SourceDiagnostic::error(error.to_string(), None),
            Self::Declaration(d) => {
                let message = match d.error {
                    DeclarationError::Unsupported { .. } => {
                        "Unsupported declaration in the ordinary-class view"
                    }
                    DeclarationError::InvalidBinding { .. } => {
                        "Inconsistent source declaration binding"
                    }
                };
                SourceDiagnostic {
                    code: self.code(),
                    ..SourceDiagnostic::error(message.into(), d.location)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
