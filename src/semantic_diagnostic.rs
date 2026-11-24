//! Owned diagnostics from semantic checks on ordinary source classes.
use crate::ReportedTyperError;
use crate::core::{SourceId, SourceSpan, Span, SymbolId};
use crate::typer::TyperDiagnostic;

/// Operation that produced a direct semantic diagnostic in this source revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticPhase {
    /// Completion of the primary constructor signature.
    Constructor,
    /// Completion of the class header, including its parent application.
    ClassHeader,
    /// Completion of a field or method signature.
    MemberSignature,
    /// Checking a field initializer or method body.
    ClassBody,
}

/// Original payload of a direct semantic diagnostic, before presentation.
#[derive(Debug)]
pub enum SemanticIssue {
    /// A typer rejection with its original error and captured source site.
    Typing(Box<ReportedTyperError>),
    /// A warning emitted by a committed typing operation.
    Warning {
        /// File containing the warning's byte range.
        source: SourceId,
        /// Typed warning and range from the original typer.
        diagnostic: TyperDiagnostic,
    },
    /// Requested class rejected structurally before typing.
    InvalidClass(SymbolId),
    /// Requested symbol rejected as a member of the selected declarations.
    InvalidMember(SymbolId),
}

/// Owned report that remains inspectable after the typing store and ASTs are
/// dropped. Symbols refer to the original session and byte offsets to the
/// original source revision. Clients choose their own messages and encoding.
#[derive(Debug)]
pub struct SemanticDiagnostic {
    /// Phase in which the client encountered the issue.
    pub phase: SemanticPhase,
    /// File declaring the requested owner, when known. A recursive error may originate elsewhere.
    pub owner_source: Option<SourceId>,
    /// Enclosing or requested class when available.
    pub class: Option<SymbolId>,
    /// Typed payload used to select a presentation.
    pub issue: SemanticIssue,
}

impl SemanticDiagnostic {
    /// Requested field, method or constructor if known. Body warnings carry class-level ownership.
    pub fn member(&self) -> Option<SymbolId> {
        match &self.issue {
            SemanticIssue::Typing(report) => report.member(),
            SemanticIssue::InvalidMember(id) => Some(*id),
            _ => None,
        }
    }

    /// Byte range captured for the issue, including its actual file. Unlocated
    /// and structural errors have no range.
    pub fn location(&self) -> Option<SourceSpan> {
        match &self.issue {
            SemanticIssue::Typing(report) => report.location(),
            SemanticIssue::Warning { source, diagnostic } => Some(SourceSpan::new(
                *source,
                Span::without_point(diagnostic.span()),
            )),
            _ => None,
        }
    }

    /// File of the reported site when known, even without a byte range.
    /// Falls back to the declaring file if the issue has no source site.
    pub fn source(&self) -> Option<SourceId> {
        match &self.issue {
            SemanticIssue::Typing(report) => report.source().or(self.owner_source),
            SemanticIssue::Warning { source, .. } => Some(*source),
            _ => self.owner_source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DiagnosticSeverity, TextRange};
    use crate::typer::{TyperError, TyperWarning};

    #[test]
    fn owner_and_unlocated_site_are_independent() {
        let owner = SourceId::from_index(2);
        let site = SourceId::from_index(9);
        let error = TyperError::ApplicationArityMismatch {
            source: site,
            tree_index: 0,
            expected: 1,
            actual: 2,
        };
        let report = SemanticDiagnostic {
            phase: SemanticPhase::MemberSignature,
            owner_source: Some(owner),
            class: None,
            issue: SemanticIssue::Typing(Box::new(ReportedTyperError::from_typer(error, |_| None))),
        };
        assert_eq!(report.source(), Some(site));
        assert_eq!(report.location(), None);
        assert!(
            matches!(report.issue, SemanticIssue::Typing(e) if matches!(e.error(), TyperError::ApplicationArityMismatch {expected:1,actual:2,..}))
        );
        let range = TextRange::new(3, 7).unwrap();
        let report = SemanticDiagnostic {
            phase: SemanticPhase::ClassBody,
            owner_source: Some(owner),
            class: None,
            issue: SemanticIssue::Warning {
                source: site,
                diagnostic: TyperDiagnostic::with_issue(
                    DiagnosticSeverity::Warning,
                    range,
                    TyperWarning::ImplicitClauseWithoutUsing,
                ),
            },
        };
        assert_eq!(report.source(), Some(site));
        assert_eq!(report.location().unwrap().span().range(), range);
        assert!(
            matches!(report.issue, SemanticIssue::Warning{diagnostic,..} if diagnostic.issue() == &TyperWarning::ImplicitClauseWithoutUsing)
        );
    }
}
