//! Scan and parse one Scala source before binding its names to a session.
use crate::SourceDiagnostic;
use crate::core::{NameInterner, SourceId, SourceText};
use crate::lexer::{ContextualScanner, RawLexerError};
use crate::parser::{DetachedParse, ParseResult, ParserFeatures};

/// Syntax with worker-local names and the reports from the same scan.
pub struct DetachedSource {
    syntax: DetachedParse,
    lexer_reports: Vec<SourceDiagnostic>,
}

/// A compilation unit with session-bound names and all its syntax diagnostics.
#[derive(Debug)]
pub struct ParsedSource {
    /// The original parser result, including its structured parser issues.
    pub syntax: ParseResult,
    /// Combined lexer and parser reports in ascending order of starting byte offset.
    /// Ties keep lexer reports first, then emission order within each phase.
    pub diagnostics: Vec<SourceDiagnostic>,
}

/// Scans and parses one file. Recoverable source errors still yield a syntax tree.
/// A worker can return the detached result to the owner of the session's interner.
/// `Err` indicates that the source could not be represented or scanned.
pub fn parse_source(
    id: SourceId,
    text: &str,
    features: ParserFeatures,
) -> Result<DetachedSource, RawLexerError> {
    let source = SourceText::new(text)?;
    let scanner = ContextualScanner::new(text)?;
    let lexer_reports = scanner
        .diagnostics()
        .iter()
        .map(|d| SourceDiagnostic::from_lexer(id, d))
        .collect();
    Ok(DetachedSource {
        syntax: DetachedParse::parse(source, id, scanner, features),
        lexer_reports,
    })
}

impl DetachedSource {
    /// Binds recovered and clean trees alike. Bind in input order for reproducible name IDs.
    pub fn into_session(self, names: &mut NameInterner) -> ParsedSource {
        let syntax = self.syntax.into_session(names);
        let mut diagnostics = self.lexer_reports;
        diagnostics.extend(syntax.diagnostics.iter().map(SourceDiagnostic::from_parser));
        diagnostics.sort_by_key(|d| d.location.map(|span| span.span().range().start()));
        ParsedSource {
            syntax,
            diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::TreeKind;

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
