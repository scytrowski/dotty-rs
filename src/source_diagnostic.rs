//! Presentation of source diagnostics for external compiler and editor clients.
use crate::core::{AstArena, DiagnosticSeverity, SourceId, SourceSpan, Span, Untyped};
use crate::lexer::LexerDiagnostic;
use crate::namer::{NamerDiagnostic, NamerIssue, VisibilityAccess};
use crate::parser::{DeclarationIssue, ParseDiagnostic, ParseIssue, ValueDefinitionKind};
use crate::typer::{TyperDiagnostic, TyperError, TyperWarning};

mod lexer;

/// A source diagnostic reported to a compiler or editor client.
#[derive(Debug, PartialEq, Eq)]
pub struct SourceDiagnostic {
    /// Stable code supplied by the originating phase, when available.
    pub code: Option<&'static str>,
    /// Severity assigned by the originating compiler phase.
    pub severity: DiagnosticSeverity,
    /// Owned text suitable for reporting after rollback.
    pub message: String,
    /// Source file and UTF-8 byte span when the error can be located.
    pub location: Option<SourceSpan>,
}

impl SourceDiagnostic {
    pub(crate) fn error(message: String, location: Option<SourceSpan>) -> Self {
        Self {
            code: None,
            severity: DiagnosticSeverity::Error,
            message,
            location,
        }
    }

    /// Presents a raw lexer or contextual scanner diagnostic for the file it came from.
    pub fn from_lexer(source: SourceId, diagnostic: &LexerDiagnostic) -> Self {
        let issue = diagnostic.issue();
        Self {
            code: Some(issue.code()),
            severity: diagnostic.severity(),
            message: lexer::message(issue),
            location: Some(SourceSpan::new(
                source,
                Span::without_point(diagnostic.span()),
            )),
        }
    }

    /// Presents a parser diagnostic with its stable issue code and original byte range.
    /// Declaration errors have specific messages. Other groups retain their code in the text.
    pub fn from_parser(diagnostic: &ParseDiagnostic) -> Self {
        let issue = diagnostic.issue();
        let message = match issue {
            ParseIssue::Declaration(declaration) => declaration_message(declaration).to_owned(),
            _ => format!("Parser diagnostic ({})", issue.code()),
        };
        Self {
            code: Some(issue.code()),
            severity: diagnostic.severity(),
            message,
            location: Some(SourceSpan::new(
                diagnostic.source(),
                Span::without_point(diagnostic.span()),
            )),
        }
    }

    /// Presents a confirmed naming issue after the naming transaction has rolled back.
    pub fn from_namer(diagnostic: &NamerDiagnostic) -> Self {
        let message = match diagnostic.issue() {
            NamerIssue::InvalidVisibilityQualifier { access, qualifier } => {
                let keyword = match access {
                    VisibilityAccess::Private => "private",
                    VisibilityAccess::Protected => "protected",
                };
                format!(
                    "Invalid '{keyword}' qualifier '{qualifier}': expected an enclosing class, object or package"
                )
            }
        };
        Self {
            code: Some(diagnostic.code()),
            severity: diagnostic.severity(),
            message,
            location: Some(SourceSpan::new(
                diagnostic.source(),
                Span::without_point(diagnostic.span()),
            )),
        }
    }

    /// Presents a committed typer warning from the supplied source file.
    pub fn from_typer_warning(source: SourceId, diagnostic: &TyperDiagnostic) -> Self {
        let message = match diagnostic.issue() {
            TyperWarning::DefaultShadowsGiven { .. } => {
                "A default argument was used for a contextual parameter with available evidence."
            }
            TyperWarning::ImplicitClauseWithoutUsing => {
                "Implicit parameters should be provided with a `using` clause."
            }
        };
        Self {
            code: Some(diagnostic.issue().code()),
            severity: diagnostic.severity(),
            message: message.into(),
            location: Some(SourceSpan::new(
                source,
                Span::without_point(diagnostic.span()),
            )),
        }
    }

    /// Reports a typer failure, including after a failed typing attempt has rolled back.
    /// Supply the original untyped arena for the requested source ID. If the source
    /// is unavailable or the error cannot be located by [`crate::typer_error_span_with`],
    /// the message remains available with `location: None`.
    pub fn from_typer<'a>(
        error: &TyperError,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Self {
        Self::typer_at(error, crate::typer_error_span_with(error, ast_for))
    }

    pub(crate) fn typer_at(error: &TyperError, location: Option<SourceSpan>) -> Self {
        let message = match error {
            TyperError::ApplicationArgumentTypeMismatch {
                expected_description,
                actual_description,
                ..
            } => format!("Expected {expected_description}, found {actual_description}"),
            TyperError::ApplicationArityMismatch {
                expected, actual, ..
            } => {
                let noun = if *expected == 1 {
                    "argument"
                } else {
                    "arguments"
                };
                format!("Expected {expected} {noun}, got {actual}")
            }
            TyperError::TypeNameNotFound { .. } => "Unknown type name".into(),
            TyperError::UnknownNamedArgument { .. } => {
                "The selected clause has no parameter with this name.".into()
            }
            TyperError::DuplicateNamedArgument { .. } => {
                "This parameter already has an argument.".into()
            }
            TyperError::PositionalAfterNamedArgument { .. } => {
                "Positional argument after an out-of-order named argument.".into()
            }
            TyperError::TermNameNotFound { .. } => "Unknown term name".into(),
            TyperError::ExpectedExpressionTypeMismatch { .. } => {
                "Expression does not conform to the expected type".into()
            }
            _ => error.to_string(),
        };
        Self {
            code: error.diagnostic_code(),
            ..Self::error(message, location)
        }
    }
}

fn declaration_message(issue: &DeclarationIssue) -> &'static str {
    use DeclarationIssue::*;
    match issue {
        ExpectedMethodSeparator { .. } => "Expected ':' for a result type or '=' for a method body",
        SecondaryConstructorOutsideTemplate => "A secondary constructor must appear inside a class",
        ExpectedMethodName { .. } => "Expected a method name after 'def'",
        ExpectedValueName {
            declaration: ValueDefinitionKind::Val,
            ..
        } => "Expected a name after 'val'",
        ExpectedValueName {
            declaration: ValueDefinitionKind::Var,
            ..
        } => "Expected a name after 'var'",
        ExpectedValueEquals { .. } => "Expected '=' before the initializer",
        ExpectedValueTypeOrEquals { .. } => "Expected a type annotation or '=' after the name",
        ExpectedPatternAfterValueKeyword {
            declaration: ValueDefinitionKind::Val,
            ..
        } => "Expected a pattern after 'val'",
        ExpectedPatternAfterValueKeyword {
            declaration: ValueDefinitionKind::Var,
            ..
        } => "Expected a pattern after 'var'",
        CommaSeparatedValuePatternsMustBeIdentifiers { .. } => {
            "Expected simple names in a comma-separated declaration"
        }
        ExpectedIdentifierAfterValuePatternComma { .. } => "Expected a name after ','",
        ExpectedEqualsAfterPatternDefinition { .. } => "Expected '=' after the pattern",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{TextRange, TokenKind};

    fn assert_lexer_report(text: &str, start: u32, end: u32, code: &str, message: &str) {
        let id = SourceId::from_index(17);
        let scanner = crate::lexer::ContextualScanner::new(text).unwrap();
        let [diagnostic] = scanner.diagnostics() else {
            panic!("expected one diagnostic for {text}")
        };
        let report = SourceDiagnostic::from_lexer(id, diagnostic);
        assert_eq!(report.code, Some(code));
        assert_eq!(report.message, message);
        assert_eq!(report.severity, DiagnosticSeverity::Error);
        let range = TextRange::new(start, end).unwrap();
        assert_eq!(
            report.location,
            Some(SourceSpan::new(id, Span::without_point(range)))
        );
    }

    #[test]
    fn typer_warning_report_preserves_code_severity_and_source_range() {
        let id = SourceId::from_index(9);
        let range = TextRange::new(17, 18).unwrap();
        let warning = TyperDiagnostic::with_issue(
            DiagnosticSeverity::Warning,
            range,
            TyperWarning::ImplicitClauseWithoutUsing,
        );
        let report = SourceDiagnostic::from_typer_warning(id, &warning);
        assert_eq!(report.code, Some("typer.implicit_clause_without_using"));
        assert_eq!(
            report.message,
            "Implicit parameters should be provided with a `using` clause."
        );
        assert_eq!(report.severity, DiagnosticSeverity::Warning);
        assert_eq!(
            report.location,
            Some(SourceSpan::new(id, Span::without_point(range)))
        );
    }

    #[test]
    fn lexer_reports_preserve_the_issue_and_scanned_byte_range() {
        for (text, start, end, code, message) in [
            (
                "/* 😀",
                0,
                7,
                "lexer.unterminated_block_comment",
                "Unterminated block comment",
            ),
            (
                r#"`a\q`"#,
                2,
                4,
                "lexer.escape.invalid_escape_character",
                "Invalid escape character 'q'",
            ),
            (
                "1e2L",
                0,
                3,
                "lexer.numeric.long_suffix_on_non_integer",
                "A literal with an exponent cannot have an 'L' suffix",
            ),
            (
                "'😀'",
                0,
                5,
                "lexer.literal.character_outside_utf16_code_unit",
                "Character '😀' does not fit in a single UTF-16 code unit",
            ),
            (
                "\"\"\"abc",
                0,
                6,
                "lexer.literal.unclosed_string",
                "Unterminated triple-quoted string literal",
            ),
            (
                r#""\123""#,
                0,
                5,
                "lexer.escape.unsupported_octal_escape",
                r"Unsupported octal escape '\123'",
            ),
            (
                "1e+",
                0,
                3,
                "lexer.numeric.missing_digits",
                "Expected a digit in the exponent",
            ),
            (
                "0x",
                0,
                2,
                "lexer.numeric.missing_digits",
                "Expected a base-16 digit after the prefix",
            ),
            (
                "0b2",
                0,
                2,
                "lexer.numeric.invalid_digit_for_base",
                "Invalid digit '2' for base 2",
            ),
        ] {
            assert_lexer_report(text, start, end, code, message);
        }
    }

    #[test]
    fn contextual_and_identifier_errors_describe_the_source() {
        assert_lexer_report(
            r#"s"$!""#,
            1,
            3,
            "lexer.interpolation.invalid_splice",
            "Expected an identifier or '{' after '$', found '!'",
        );
        assert_lexer_report(
            "if ready then\n  first\n\tsecond",
            22,
            23,
            "lexer.incompatible_indentation_prefixes",
            "Inconsistent indentation: spaces and tabs give incompatible prefixes",
        );
        assert_lexer_report(
            "\u{1a}",
            0,
            1,
            "lexer.unsupported_source_character",
            r"Unsupported source character '\u{1a}'",
        );
        assert_lexer_report(
            "`_`",
            0,
            3,
            "lexer.empty_or_underscore_backquoted_identifier",
            "A backquoted identifier cannot be empty or contain only '_'",
        );
    }

    #[test]
    fn xml_reports_keep_tag_names_and_empty_eof_spans() {
        assert_lexer_report(
            "<outer></inner>",
            14,
            15,
            "lexer.xml.mismatched_closing_tag",
            "Expected </outer>, found </inner>",
        );
        assert_lexer_report(
            "<item enabled={flag",
            19,
            19,
            "lexer.xml.attribute_expression_must_be_closed",
            "Expected '}' to close the XML attribute expression",
        );
    }

    #[test]
    fn parser_reports_preserve_the_phase_code_and_source_location() {
        let span = SourceSpan::new(
            SourceId::from_index(4),
            Span::without_point(TextRange::new(7, 8).unwrap()),
        );
        let found = TokenKind::Eof;
        for (issue, code, message) in [
            (
                ParseIssue::Declaration(DeclarationIssue::ExpectedValueEquals { found }),
                "parser.declaration.expected_value_equals",
                "Expected '=' before the initializer",
            ),
            (
                ParseIssue::Declaration(DeclarationIssue::ExpectedMethodName { found }),
                "parser.declaration.expected_method_name",
                "Expected a method name after 'def'",
            ),
            (
                ParseIssue::ExpectedExpression { found },
                "parser.expected_expression",
                "Parser diagnostic (parser.expected_expression)",
            ),
        ] {
            let report = SourceDiagnostic::from_parser(&ParseDiagnostic::with_issue(span, issue));
            assert_eq!(report.code, Some(code));
            assert_eq!(report.message, message);
            assert_eq!(report.location, Some(span));
            assert_eq!(report.severity, DiagnosticSeverity::Error);
        }
    }

    #[test]
    fn naming_reports_preserve_visibility_kind_and_qualifier_location() {
        let span = SourceSpan::new(
            SourceId::from_index(9),
            Span::without_point(TextRange::new(12, 19).unwrap()),
        );
        for (access, keyword) in [
            (VisibilityAccess::Private, "private"),
            (VisibilityAccess::Protected, "protected"),
        ] {
            let d = NamerDiagnostic::error(
                span,
                NamerIssue::InvalidVisibilityQualifier {
                    access,
                    qualifier: "Missing".to_owned(),
                },
            );
            let report = SourceDiagnostic::from_namer(&d);
            assert_eq!(report.code, Some("namer.visibility.invalid_qualifier"));
            assert_eq!(
                report.message,
                format!(
                    "Invalid '{keyword}' qualifier 'Missing': expected an enclosing class, object or package"
                )
            );
            assert_eq!(report.severity, DiagnosticSeverity::Error);
            assert_eq!(report.location, Some(span));
        }
    }
}
