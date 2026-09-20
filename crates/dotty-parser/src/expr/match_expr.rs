use dotty_core::ast::Match;
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Mark, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the case region following an already parsed match selector.
    pub(super) fn parse_match_clause(&mut self, selector: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(selector)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let mark = Mark { start };

        self.advance();
        let cases = if self.accept(TokenKind::Punctuation(Punctuation::LeftBrace)) {
            let cases = self.case_clauses();
            if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `}` to close match cases",
                );
            }
            cases
        } else if self.features().sub_cases
            && self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Case)
        {
            vec![self.case_clause(true)]
        } else {
            self.consume_match_newlines();
            if self.accept(TokenKind::Indent) {
                let cases = self.case_clauses();
                if !self.accept(TokenKind::Outdent) {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedToken,
                        "expected an outdent to close match cases",
                    );
                }
                cases
            } else {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `{` or an indented case region after `match`",
                );
                self.case_clauses()
            }
        };

        if cases.is_empty() {
            self.report(
                crate::ParseDiagnosticKind::ExpectedPattern,
                "expected at least one `case` clause after `match`",
            );
        }

        self.alloc_from(mark, TreeKind::Match(Match { selector, cases }))
    }

    fn consume_match_newlines(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{CaseDef, InfixOp, Match as MatchTree, UntypedNode};
    use dotty_core::{HardKeyword, NameInterner, TextRange};

    #[test]
    fn parses_a_braced_match_with_a_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "value match { case x => x }",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Case), 14, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::Match(MatchTree {
            selector,
            ref cases,
        }) = parser.ast().get(tree).kind
        else {
            panic!("expected match tree");
        };
        assert!(matches!(
            parser.ast().get(selector).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(cases.len(), 1);
        assert!(matches!(
            parser.ast().get(cases[0]).kind,
            TreeKind::CaseDef(CaseDef { .. })
        ));
        assert_eq!(
            parser.ast().get(tree).position.unwrap().span().range(),
            TextRange::new(0, 27).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn match_uses_the_complete_infix_selector() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "a + b match { case x => x }",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Case), 14, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::Match(MatchTree { selector, .. }) = parser.ast().get(tree).kind else {
            panic!("expected match tree");
        };
        assert!(matches!(
            parser.ast().get(selector).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(InfixOp { .. }))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_indented_match_case_region() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "value match\n  case x => x",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Indent, 14, 14),
                token(TokenKind::Keyword(HardKeyword::Case), 14, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Outdent, 25, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let tree = parser.expr();

        assert!(matches!(parser.ast().get(tree).kind, TreeKind::Match(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_single_case_without_braces_or_indentation() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "value match case x => x",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Keyword(HardKeyword::Case), 12, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            sub_cases: true,
            ..crate::ParserFeatures::default()
        });

        let tree = parser.expr();
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(tree).kind else {
            panic!("expected match tree");
        };
        assert_eq!(cases.len(), 1);
        let TreeKind::CaseDef(CaseDef { pattern, body, .. }) = parser.ast().get(cases[0]).kind
        else {
            panic!("expected case definition");
        };
        assert!(matches!(parser.ast().get(pattern).kind, TreeKind::Ident(_)));
        assert!(matches!(parser.ast().get(body).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.ast().get(tree).position.unwrap().span().range(),
            TextRange::new(0, 23).unwrap()
        );
        assert_eq!(
            parser.ast().get(cases[0]).position.unwrap().span().range(),
            TextRange::new(12, 23).unwrap()
        );
        assert_eq!(
            parser.ast().get(body).position.unwrap().span().range(),
            TextRange::new(22, 23).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_single_case_without_braces_or_indentation_by_default() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "value match case x => x",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Keyword(HardKeyword::Case), 12, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let tree = parser.expr();

        assert!(matches!(parser.ast().get(tree).kind, TreeKind::Match(_)));
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_an_empty_match_case_region() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "value match {}",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let tree = parser.expr();

        assert!(matches!(parser.ast().get(tree).kind, TreeKind::Match(_)));
        assert_eq!(parser.diagnostics().len(), 1);
    }
}
