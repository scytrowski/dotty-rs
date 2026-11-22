//! Owned application failures from the source typer.
use crate::core::{AstArena, SourceId, SourceSpan, Untyped};
use crate::typer::TyperError;
use crate::{SourceDiagnostic, typer_error_span_with};

/// An original typed call failure with the source position resolved during analysis.
/// It can outlive the typer and both arenas. Type IDs inside the error describe
/// the failed attempt and may have been rolled back. Its owned descriptions remain valid.
#[derive(Debug)]
pub struct CallDiagnostic {
    error: TyperError,
    source: SourceId,
    location: Option<SourceSpan>,
}

impl CallDiagnostic {
    /// Takes ownership of an arity or argument type mismatch. Other errors are
    /// returned intact without requesting a tree. Supply the original untyped
    /// arena for the requested source, or `None` to retain an unlocated failure.
    pub fn from_typer<'a>(
        error: TyperError,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Result<Self, TyperError> {
        let source = match error {
            TyperError::ApplicationArityMismatch { source, .. }
            | TyperError::ApplicationArgumentTypeMismatch { source, .. } => source,
            _ => return Err(error),
        };
        let location = typer_error_span_with(&error, ast_for);
        Ok(Self {
            error,
            source,
            location,
        })
    }

    /// Original payload, including argument index or arity counts.
    pub fn error(&self) -> &TyperError {
        &self.error
    }

    /// Stable machine-readable category, independent of the argument types and counts.
    pub fn code(&self) -> &'static str {
        self.error
            .call_error_code()
            .expect("validated call failure")
    }

    /// Original file identity, available even without its tree.
    pub fn source(&self) -> SourceId {
        self.source
    }

    /// UTF-8 byte span of the call for arity, or the rejected argument for type mismatch.
    pub fn location(&self) -> Option<SourceSpan> {
        self.location
    }

    /// Formats the default message without querying a source or semantic arena.
    pub fn render(&self) -> SourceDiagnostic {
        SourceDiagnostic::typer_at(&self.error, self.location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Definitions, NameInterner, SemanticStore, TreeKind};

    #[test]
    fn retains_payload_and_source_with_or_without_the_original_tree() {
        let source = SourceId::from_index(3);
        let text = "class A { def f() = only(false) }";
        let parsed = crate::parse_source(source, text, Default::default())
            .unwrap()
            .bind(&mut NameInterner::new());
        let ast = &parsed.syntax.ast;
        let (call, _) = ast
            .iter()
            .find(|(_, tree)| matches!(tree.kind, TreeKind::Apply(_)))
            .unwrap();
        let defs = Definitions::bootstrap(&mut SemanticStore::new());
        for available in [true, false] {
            for (error, site) in [
                (
                    TyperError::ApplicationArityMismatch {
                        source,
                        tree_index: call.index(),
                        expected: 0,
                        actual: 1,
                    },
                    "only(false)",
                ),
                (
                    TyperError::ApplicationArgumentTypeMismatch {
                        source,
                        tree_index: call.index(),
                        argument_index: 0,
                        expected: defs.int,
                        actual: defs.boolean,
                        expected_description: "Int".into(),
                        actual_description: "Boolean".into(),
                    },
                    "false",
                ),
            ] {
                let lookup = |id| {
                    assert_eq!(id, source);
                    available.then_some(ast)
                };
                let rendered = SourceDiagnostic::from_typer(&error, lookup);
                let d = CallDiagnostic::from_typer(error, lookup).unwrap();
                assert_eq!(d.source(), source);
                assert_eq!(d.error().call_error_code(), Some(d.code()));
                assert_eq!(d.render(), rendered);
                assert_eq!(
                    d.location().map(|span| {
                        assert_eq!(span.source(), source);
                        let range = span.span().range();
                        &text[range.start() as usize..range.end() as usize]
                    }),
                    available.then_some(site)
                );
            }
        }
        assert!(matches!(
            CallDiagnostic::from_typer(
                TyperError::UsingApplicationDeferred {
                    source,
                    tree_index: call.index()
                },
                |_| panic!("unsupported error queried an AST")
            ),
            Err(TyperError::UsingApplicationDeferred { .. })
        ));
    }
}
