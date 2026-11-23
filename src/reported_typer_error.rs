//! Typer failures retained by clients after a source analysis.
use crate::core::{AstArena, SourceId, SourceSpan, SymbolId, Untyped};
use crate::typer::TyperError;
use crate::{CallDiagnostic, typer_error_span_with};

/// An owned typer failure with its location resolved against the original parse.
/// The error and byte span can outlive the analysis. IDs in the payload belong
/// to that analysis only, and IDs created during failed attempts may already
/// have been rolled back. Apply the span to the same version of the source text.
#[derive(Debug)]
pub struct ReportedTyperError {
    failure: Failure,
    class: Option<SymbolId>,
    member: Option<SymbolId>,
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
        ast_for: impl FnMut(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Self {
        Self::capture(error, None, None, ast_for)
    }

    /// Captures a typer failure encountered while processing a known class.
    /// Supply the same-session class and, if known, its constructor, field or method.
    /// This records caller-provided ownership independently of the error location.
    pub fn from_class<'a>(
        error: TyperError,
        class: SymbolId,
        member: Option<SymbolId>,
        ast_for: impl FnMut(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Self {
        Self::capture(error, Some(class), member, ast_for)
    }

    fn capture<'a>(
        error: TyperError,
        class: Option<SymbolId>,
        member: Option<SymbolId>,
        mut ast_for: impl FnMut(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Self {
        let failure = match CallDiagnostic::from_typer(error, member, &mut ast_for) {
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
        Self {
            failure,
            class,
            member,
        }
    }

    /// Canonical enclosing class if supplied by the phase or caller.
    pub fn class(&self) -> Option<SymbolId> {
        self.class
    }

    /// Canonical member if supplied by the phase or caller.
    pub fn member(&self) -> Option<SymbolId> {
        self.member
    }

    /// Within the same analysis, identifies a repeated arity or argument failure
    /// only when [`CallDiagnostic::identity`] can establish a safe key. Keep
    /// both reports if this returns `None`; discard all keys after an edit.
    pub fn call_identity(
        &self,
        definitions: &crate::core::Definitions,
    ) -> Option<crate::CallErrorIdentity> {
        match &self.failure {
            Failure::Call(call) => call.identity(definitions),
            Failure::Other { .. } => None,
        }
    }

    /// Stable category when the typer assigns one. Always inspect [`Self::error`]
    /// for the full payload, including for variants without a code.
    pub fn code(&self) -> Option<&'static str> {
        self.error().diagnostic_code()
    }

    /// Builds the default presentation on demand without consulting session stores.
    pub fn render(&self) -> crate::SourceDiagnostic {
        crate::SourceDiagnostic::typer_at(self.error(), self.location())
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
            assert_eq!(report.code(), Some("typer.type_name_not_found"));
            let rendered = report.render();
            assert_eq!(rendered.code, report.code());
            assert_eq!(rendered.location, report.location());
            assert_eq!(rendered.message, "Unknown type name");
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

    #[test]
    fn body_error_codes_are_available_without_rendering() {
        use crate::core::NoResolver;
        for (expr, code, message) in [
            (
                "missingValue",
                "typer.term_name_not_found",
                "Unknown term name",
            ),
            (
                "false",
                "typer.expected_expression_type_mismatch",
                "Expression does not conform to the expected type",
            ),
        ] {
            let id = SourceId::from_index(42);
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let file = crate::parse_source(
                id,
                &format!("class C {{ def n: Int = {expr} }}"),
                Default::default(),
            )
            .unwrap()
            .bind(&mut store.names)
            .name("C.scala", &mut store, &mut packages)
            .unwrap()
            .into_classes(&store)
            .unwrap();
            let class = &file.classes()[0];
            let mut typer =
                SourceTyper::new(file.ast(), id, file.index(), &mut store, defs, &packages);
            for s in [
                class.constructor,
                class.symbol,
                class.methods().next().unwrap().symbol,
            ] {
                typer.complete_symbol(s).unwrap();
            }
            drop(typer);
            let failure = crate::check_class_body(
                class,
                file.ast(),
                file.index(),
                &mut store,
                defs,
                &packages,
                &mut AstArena::new(),
                NoResolver,
            )
            .body
            .unwrap_err()
            .pop()
            .unwrap();
            let crate::ClassBodyError::Member { error, .. } = failure else {
                panic!("expected a typing failure")
            };
            let report = ReportedTyperError::from_typer(error, |_| Some(file.ast()));
            assert!(matches!(
                (expr, report.error()),
                ("false", TyperError::ExpectedExpressionTypeMismatch { .. })
                    | ("missingValue", TyperError::TermNameNotFound { .. })
            ));
            assert_eq!(report.code(), Some(code));
            assert_eq!(report.render().message, message);
        }
    }

    #[test]
    fn each_phase_reports_the_declaring_field_and_returns_structural_errors_intact() {
        use crate::{
            ClassBodyError, ClassFieldSignatureError, MemberSignatureError, NamedClassSources,
            core::NoResolver,
        };
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let id = SourceId::from_index(7);
        let text = "class A { val answer = 40 }; class B { val answer = missingValue }";
        let file = crate::parse_source(id, text, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("Both.scala", &mut store, &mut packages)
            .unwrap()
            .into_classes(&store)
            .unwrap();
        let sources = NamedClassSources::new([&file], &store).unwrap();
        let mut arena = AstArena::new();
        let mut bindings = Default::default();
        for (_, class) in sources.classes() {
            SourceTyper::new(file.ast(), id, file.index(), &mut store, defs, &packages)
                .complete_symbol(class.constructor)
                .unwrap();
            let header = sources
                .try_complete_class_header(
                    class.symbol,
                    &mut store,
                    defs,
                    &packages,
                    &mut arena,
                    &mut bindings,
                )
                .unwrap();
            assert!(matches!(
                header,
                crate::typer::SignatureCompletion::Complete(_)
            ));
        }
        let class = &file.classes()[1];
        let field = class.fields().next().unwrap().symbol;
        assert_ne!(field, file.classes()[0].fields().next().unwrap().symbol);
        let lookup = |id| sources.file(id).map(crate::NamedClassSource::ast);
        let member = sources
            .try_complete_member_signature(field, &mut store, defs, &packages)
            .unwrap_err()
            .into_reported_error(lookup)
            .unwrap();
        let group = crate::try_complete_class_field_signatures(
            class.symbol,
            file.ast(),
            file.index(),
            &mut store,
            defs,
            &packages,
            |_| false,
        )
        .unwrap_err()
        .into_reported_error(lookup)
        .unwrap();
        let body = sources
            .type_class_body(
                class.symbol,
                &mut store,
                defs,
                &packages,
                &mut arena,
                NoResolver,
            )
            .unwrap_err()
            .into_reported_error(lookup)
            .unwrap();
        for report in [member, group, body] {
            assert_eq!(
                (report.class(), report.member()),
                (Some(class.symbol), Some(field))
            );
            assert!(matches!(
                report.error(),
                TyperError::TermNameNotFound { .. }
            ));
            let span = report.location().unwrap();
            assert_eq!(span.source(), id);
            let range = span.span().range();
            assert_eq!(
                &text[range.start() as usize..range.end() as usize],
                "missingValue"
            );
        }
        assert!(
            matches!(MemberSignatureError::InvalidMember(field).into_reported_error(|_|unreachable!()),Err(MemberSignatureError::InvalidMember(s)) if s == field)
        );
        assert!(
            matches!(ClassBodyError::InvalidClass(class.symbol).into_reported_error(|_|unreachable!()),Err(ClassBodyError::InvalidClass(s)) if s == class.symbol)
        );
        assert!(
            matches!(ClassFieldSignatureError::InvalidClass(class.symbol).into_reported_error(|_|unreachable!()),Err(ClassFieldSignatureError::InvalidClass(s)) if s == class.symbol)
        );
    }

    #[test]
    fn retries_share_a_call_key_only_at_the_same_known_call_site() {
        use crate::core::TreeKind;
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(3);
        let file = crate::parse_source(
            source,
            "class C { def f() = { value(1); value(1) } }",
            Default::default(),
        )
        .unwrap()
        .bind(&mut store.names)
        .name("C.scala", &mut store, &mut Packages::new())
        .unwrap()
        .into_classes(&store)
        .unwrap();
        let c = &file.classes()[0];
        let member = c.methods().next().unwrap().symbol;
        let sites: Vec<_> = file
            .ast()
            .iter()
            .filter(|(_, tree)| matches!(tree.kind, TreeKind::Apply(_)))
            .map(|(id, _)| id.index())
            .collect();
        assert_eq!(sites.len(), 2);
        let error = |tree_index| TyperError::ApplicationArityMismatch {
            source,
            tree_index,
            actual: 1,
            expected: 0,
        };
        let signature = crate::MemberSignatureError::Typing {
            member,
            declaration: crate::SourceDeclaration {
                source,
                class: c.symbol,
                kind: crate::core::SymbolKind::Method,
            },
            error: error(sites[0]),
        }
        .into_reported_error(|_| Some(file.ast()))
        .unwrap();
        let body = |site| {
            crate::ClassBodyError::Member {
                class: c.symbol,
                member: c.members[0],
                error: error(site),
            }
            .into_reported_error(|_| Some(file.ast()))
            .unwrap()
        };
        let first = signature.call_identity(&defs).unwrap();
        assert_eq!(body(sites[0]).call_identity(&defs), Some(first));
        assert_ne!(body(sites[1]).call_identity(&defs).unwrap(), first);
        assert_eq!(
            ReportedTyperError::from_typer(error(sites[0]), |_| Some(file.ast()))
                .call_identity(&defs),
            None
        );
        assert_eq!(
            ReportedTyperError::from_class(error(sites[0]), c.symbol, Some(member), |_| None)
                .call_identity(&defs),
            None
        );
        assert_eq!(
            ReportedTyperError::from_class(
                TyperError::UnknownSymbol { symbol: member },
                c.symbol,
                Some(member),
                |_| unreachable!()
            )
            .call_identity(&defs),
            None
        );
    }
}
