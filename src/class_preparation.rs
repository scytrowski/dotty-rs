//! Preparing an individual bound source for ordinary-class analysis.
use crate::core::{Packages, SemanticStore, SourceId, SourceSpan, Span};
use crate::namer::{NamerDiagnostic, NamerError};
use crate::source_declarations::{DeclarationDiagnostic, DeclarationError};
use crate::{
    NameSourceError, NamedClassSource, NamedSource, ParsedSource, SourceDiagnostic,
    SyntaxDiagnostic,
};

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

/// Outcome of preparing one bound file. A prepared file may have zero classes
/// or report omitted siblings. A rejected file supplies no class view.
#[derive(Debug)]
#[must_use = "inspect preparation diagnostics before proceeding"]
pub enum ClassPreparation {
    /// Naming and projection produced a view suitable for [`crate::NamedClassSources`].
    Prepared {
        /// Original named file with all complete ordinary classes in source order.
        file: NamedClassSource,
        /// Committed naming diagnostics, then projection rejections in source order.
        diagnostics: Vec<PreparationDiagnostic>,
    },
    /// Syntax, naming or a fatal projection error prevented preparation.
    Rejected {
        /// Original file ID, independent of diagnostic positions.
        source: SourceId,
        /// Findings from the failed phase and any preceding naming diagnostics.
        diagnostics: Vec<PreparationDiagnostic>,
    },
}

impl ParsedSource<SyntaxDiagnostic> {
    /// Names this already bound parse and projects ordinary classes in the same
    /// store. Supply the original filename and naming session's packages.
    /// Syntax issues skip naming; a rejected naming transaction rolls back.
    /// Projection leaves successful naming state intact, including omitted
    /// declarations. A structural projection failure returns no partial view.
    ///
    /// The result owns its typed diagnostics and never formats messages.
    /// Use [`Self::name`] directly when a caller needs the recovered syntax
    /// tree after rejection.
    pub fn prepare_classes(
        self,
        filename: &str,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> ClassPreparation {
        let source = self.syntax.call_names.source();
        let diagnostics = match self.name(filename, store, packages) {
            Ok(file) => return ClassPreparation::from_named(file, store),
            Err(NameSourceError::Syntax(parsed)) => parsed
                .diagnostics
                .into_iter()
                .map(PreparationDiagnostic::Syntax)
                .collect(),
            Err(NameSourceError::Rejected(issues)) => issues
                .into_iter()
                .map(PreparationDiagnostic::Naming)
                .collect(),
            Err(NameSourceError::Namer { source, error }) => {
                vec![PreparationDiagnostic::NamerFailure { source, error }]
            }
        };
        ClassPreparation::Rejected {
            source,
            diagnostics,
        }
    }
}

impl ClassPreparation {
    pub(crate) fn from_named(named: NamedSource, store: &SemanticStore) -> Self {
        let source = named.source();
        let mut diagnostics: Vec<_> = named
            .diagnostics()
            .iter()
            .cloned()
            .map(PreparationDiagnostic::Naming)
            .collect();
        match named.project_classes(store) {
            Ok(projection) => {
                diagnostics.extend(
                    projection
                        .rejections
                        .into_iter()
                        .map(PreparationDiagnostic::Declaration),
                );
                Self::Prepared {
                    file: projection.file,
                    diagnostics,
                }
            }
            Err(failure) => {
                diagnostics.push(PreparationDiagnostic::Declaration(
                    failure.error.diagnostic(&failure.named.syntax().ast),
                ));
                Self::Rejected {
                    source,
                    diagnostics,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
