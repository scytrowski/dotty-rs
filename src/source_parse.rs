//! Scan and parse one Scala source before binding its names to a session.
use crate::SourceDiagnostic;
use crate::core::{NameInterner, SourceId, SourceSpan, SourceText, Span};
use crate::lexer::{ContextualScanner, LexerDiagnostic, RawLexerError};
use crate::parser::{DetachedParse, ParseDiagnostic, ParseResult, ParserFeatures};

/// Syntax with worker-local names and the reports from the same scan.
pub struct DetachedSource {
    syntax: DetachedParse,
    source: SourceId,
    lexer_diagnostics: Vec<LexerDiagnostic>,
}

/// A syntax diagnostic owned by the caller. Match its payload before rendering text.
/// Names in parser diagnostics refer to the interner used to bind this file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyntaxDiagnostic {
    /// A raw lexer or contextual scanner issue with the identity of its source file.
    Lexer {
        /// File whose UTF-8 byte ranges are used by the lexer diagnostic.
        source: SourceId,
        /// Original typed lexer payload, retained before rendering a message.
        diagnostic: LexerDiagnostic,
    },
    /// An original parser issue whose names are bound to the session interner.
    Parser(ParseDiagnostic),
}

impl SyntaxDiagnostic {
    /// Source file and UTF-8 byte range supplied by the originating phase.
    pub fn location(&self) -> SourceSpan {
        let (source, range) = match self {
            Self::Lexer { source, diagnostic } => (*source, diagnostic.span()),
            Self::Parser(diagnostic) => (diagnostic.source(), diagnostic.span()),
        };
        SourceSpan::new(source, Span::without_point(range))
    }

    /// Formats an owned report for a terminal, editor or other presentation client.
    pub fn render(&self) -> SourceDiagnostic {
        match self {
            Self::Lexer { source, diagnostic } => SourceDiagnostic::from_lexer(*source, diagnostic),
            Self::Parser(diagnostic) => SourceDiagnostic::from_parser(diagnostic),
        }
    }
}

/// A compilation unit with session-bound names and all its syntax diagnostics.
///
/// [`DetachedSource::bind`] retains typed [`SyntaxDiagnostic`] payloads;
/// [`DetachedSource::into_session`] returns rendered [`SourceDiagnostic`] reports.
#[derive(Debug)]
pub struct ParsedSource<D = SourceDiagnostic> {
    /// The original parser result, including its structured parser issues.
    pub syntax: ParseResult,
    /// Combined lexer and parser reports in ascending order of starting byte offset.
    /// Ties keep lexer reports first, then emission order within each phase.
    pub diagnostics: Vec<D>,
}

/// Scans and parses one file. Recoverable source errors still yield a syntax tree.
/// A worker can return the detached result to the owner of the session's interner.
/// Use [`DetachedSource::bind`] to inspect [`SyntaxDiagnostic`] values or render them on demand.
/// `Err` indicates that the source could not be represented or scanned.
pub fn parse_source(
    id: SourceId,
    text: &str,
    features: ParserFeatures,
) -> Result<DetachedSource, RawLexerError> {
    let source = SourceText::new(text)?;
    let mut scanner = ContextualScanner::new(text)?;
    let lexer_diagnostics = scanner.take_diagnostics();
    Ok(DetachedSource {
        syntax: DetachedParse::parse(source, id, scanner, features),
        source: id,
        lexer_diagnostics,
    })
}

impl DetachedSource {
    /// Binds syntax and structured diagnostics without formatting messages.
    /// Bind files in input order for reproducible name IDs.
    pub fn bind(self, names: &mut NameInterner) -> ParsedSource<SyntaxDiagnostic> {
        let syntax = self.syntax.into_session(names);
        let mut diagnostics: Vec<_> = self
            .lexer_diagnostics
            .into_iter()
            .map(|diagnostic| SyntaxDiagnostic::Lexer {
                source: self.source,
                diagnostic,
            })
            .chain(
                syntax
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(SyntaxDiagnostic::Parser),
            )
            .collect();
        diagnostics.sort_by_key(|d| d.location().span().range().start());
        ParsedSource {
            syntax,
            diagnostics,
        }
    }

    /// Binds syntax and renders all diagnostics. Use [`Self::bind`] for typed diagnostics.
    pub fn into_session(self, names: &mut NameInterner) -> ParsedSource {
        let parsed = self.bind(names);
        ParsedSource {
            syntax: parsed.syntax,
            diagnostics: parsed
                .diagnostics
                .iter()
                .map(SyntaxDiagnostic::render)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::TreeKind;

    #[test]
    fn bound_syntax_exposes_original_payloads_before_presentation() {
        use crate::lexer::{LexerIssue, NumericComponent, NumericIssue};
        use crate::parser::{DeclarationIssue, ParseIssue};

        let id = SourceId::from_index(7);
        for (literal, component) in [
            ("0x", NumericComponent::BasePrefix { base: 16 }),
            ("1e+", NumericComponent::Exponent),
        ] {
            let text = format!("val x: Int {literal}");
            let parsed = parse_source(id, &text, ParserFeatures::default())
                .unwrap()
                .bind(&mut NameInterner::new());
            let diagnostics = parsed.diagnostics;
            let [
                SyntaxDiagnostic::Lexer { source, diagnostic },
                SyntaxDiagnostic::Parser(parser),
            ] = diagnostics.as_slice()
            else {
                panic!("{diagnostics:?}");
            };
            assert_eq!(*source, id);
            assert_eq!(
                diagnostic.issue(),
                &LexerIssue::Numeric(NumericIssue::MissingDigits { component })
            );
            assert!(matches!(
                parser.issue(),
                ParseIssue::Declaration(DeclarationIssue::ExpectedValueEquals { .. })
            ));
            assert_eq!(parser, &parsed.syntax.diagnostics[0]);
            assert_eq!(diagnostics[0].location(), diagnostics[1].location());
        }
    }

    #[test]
    fn parse_keeps_both_phases_and_recovers_the_class() {
        let id = SourceId::from_index(8);
        let text = "class Broken { val hidden: Int 1 } /* open";
        let mut names = NameInterner::new();
        names.intern("already bound");
        let parsed = parse_source(id, text, ParserFeatures::default())
            .unwrap()
            .into_session(&mut names);
        let [parser, lexer] = parsed.diagnostics.as_slice() else {
            panic!("{:?}", parsed.diagnostics)
        };
        for (report, code, site) in [
            (parser, "parser.declaration.expected_value_equals", "1"),
            (lexer, "lexer.unterminated_block_comment", "/* open"),
        ] {
            assert_eq!(report.code, Some(code));
            let span = report.location.unwrap();
            assert_eq!(span.source(), id);
            assert_eq!(
                SourceText::new(text)
                    .unwrap()
                    .slice(span.span().range())
                    .unwrap(),
                site
            );
        }
        let syntax = parsed.syntax;
        let TreeKind::PackageDef(root) = &syntax.ast.get(syntax.root).kind else {
            panic!("expected package")
        };
        let TreeKind::TypeDef(class) = &syntax.ast.get(root.stats[0]).kind else {
            panic!("expected class")
        };
        assert_eq!(names.resolve(class.name.as_name().text()), "Broken");
    }

    #[test]
    fn lexer_only_failure_is_visible_with_a_clean_parser_result() {
        let parsed = parse_source(
            SourceId::from_index(2),
            "class A {} /*",
            ParserFeatures::default(),
        )
        .unwrap()
        .into_session(&mut NameInterner::new());
        assert!(parsed.syntax.diagnostics.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(
            parsed.diagnostics[0].code,
            Some("lexer.unterminated_block_comment")
        );
    }

    #[test]
    fn same_offset_keeps_both_diagnostics_in_phase_order() {
        let parsed = parse_source(
            SourceId::from_index(2),
            "val n: Int 0b2",
            ParserFeatures::default(),
        )
        .unwrap()
        .into_session(&mut NameInterner::new());
        let [lexer, parser] = parsed.diagnostics.as_slice() else {
            panic!("{:?}", parsed.diagnostics)
        };
        assert_eq!(lexer.code, Some("lexer.numeric.invalid_digit_for_base"));
        assert_eq!(
            parser.code,
            Some("parser.declaration.expected_value_equals")
        );
        assert_eq!(
            lexer.location.unwrap().span().range().start(),
            parser.location.unwrap().span().range().start()
        );
    }
}
