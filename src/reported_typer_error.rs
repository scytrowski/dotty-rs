//! Typer failures retained by clients after a source analysis.
use crate::core::{AstArena, SourceId, SourceSpan, Untyped};
use crate::typer::TyperError;
use crate::{CallDiagnostic, typer_error_span_with};

/// An owned typer failure with its location resolved against the original parse.
/// The error and byte span can outlive the analysis. IDs in the payload belong
/// to that analysis only, and IDs created during failed attempts may already
/// have been rolled back. Apply the span to the same version of the source text.
#[derive(Debug)]
pub struct ReportedTyperError {
    failure: Failure,
}

#[derive(Debug)]
enum Failure {
    Call(CallDiagnostic),
    Other {
        error: TyperError,
        source: Option<SourceId>,
        location: Option<SourceSpan>,
    },
}

impl ReportedTyperError {
    /// Captures the original failure without formatting. The lookup is called
    /// at most once, for errors with a supported source site. Return the original
    /// AST from this session, or `None` to keep the failure without a span.
    pub fn from_typer<'a>(
        error: TyperError,
        mut ast_for: impl FnMut(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Self {
        let failure = match CallDiagnostic::from_typer(error, None, &mut ast_for) {
            Ok(call) => Failure::Call(call),
            Err(error) => {
                let mut source = None;
                let location = typer_error_span_with(&error, |id| {
                    source = Some(id);
                    ast_for(id)
                });
                Failure::Other {
                    error,
                    source,
                    location,
                }
            }
        };
        Self { failure }
    }

    /// Borrows the original ADT, including any context owned by the typer.
    pub fn error(&self) -> &TyperError {
        match &self.failure {
            Failure::Call(call) => call.error(),
            Failure::Other { error, .. } => error,
        }
    }

    /// File requested by the source locator, even when its AST was unavailable.
    /// Errors without a supported source site may return `None`.
    pub fn source(&self) -> Option<SourceId> {
        match &self.failure {
            Failure::Call(call) => Some(call.source()),
            Failure::Other { source, .. } => *source,
        }
    }

    /// UTF-8 byte span validated against the original AST during capture.
    pub fn location(&self) -> Option<SourceSpan> {
        match &self.failure {
            Failure::Call(call) => call.location(),
            Failure::Other { location, .. } => *location,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Definitions, Packages, SemanticStore};
    use crate::typer::SourceTyper;

    #[test]
    fn missing_type_survives_its_session_with_only_an_authenticated_span() {
        let text = "class Broken(start: Missing)";
        let id = SourceId::from_index(7);
        for available in [Some(id), Some(SourceId::from_index(42)), None] {
            let report = {
                let mut store = SemanticStore::new();
                let defs = Definitions::bootstrap(&mut store);
                let mut packages = Packages::new();
                let file = crate::parse_source(id, text, Default::default())
                    .unwrap()
                    .bind(&mut store.names)
                    .name("Broken.scala", &mut store, &mut packages)
                    .unwrap()
                    .into_classes(&store)
                    .unwrap();
                let error =
                    SourceTyper::new(file.ast(), id, file.index(), &mut store, defs, &packages)
                        .complete_symbol(file.classes()[0].constructor)
                        .unwrap_err();
                let input = available.map(|source| {
                    crate::parse_source(source, text, Default::default())
                        .unwrap()
                        .bind(&mut store.names)
                });
                let mut calls = 0;
                let report = ReportedTyperError::from_typer(error, |requested| {
                    assert_eq!(requested, id);
                    calls += 1;
                    if available == Some(id) {
                        Some(file.ast())
                    } else {
                        input.as_ref().map(|p| &p.syntax.ast)
                    }
                });
                assert_eq!(calls, 1);
                report
            };
            assert!(
                matches!(report.error(),TyperError::TypeNameNotFound { source,.. } if *source == id)
            );
            assert_eq!(report.source(), Some(id));
            assert_eq!(
                report.location().map(|s| {
                    assert_eq!(s.source(), id);
                    let r = s.span().range();
                    &text[r.start() as usize..r.end() as usize]
                }),
                (available == Some(id)).then_some("Missing")
            );
        }
    }
}
