use dotty_core::ast::Match;
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Mark, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn parse_colon_case_argument(&mut self) -> TreeId<Untyped> {
        let start =
            if self.cursor.lookahead(1).kind == TokenKind::Keyword(dotty_core::HardKeyword::Case) {
                self.cursor.lookahead(1).span.start()
            } else {
                self.cursor.lookahead(2).span.start()
            };
        let feedback_indent = self.observe_indented_body_region();
        self.advance();
        let _ = self.accept(TokenKind::Indent);

        let cases = self.case_clauses();
        if let Some(indent_offset) = feedback_indent {
            // An outdent at this point may close a nested case body. Ask the
            // scanner to close this case region as well; it will place the
            // matching delimiter before an existing nested outdent when
            // necessary.
            self.observe_outdented_region(indent_offset);
        } else if !self.cursor.at(TokenKind::Outdent) {
            self.observe_outdented();
        }
        if !self.accept(TokenKind::Outdent) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected an outdent to close colon case clauses",
            );
        }

        let selector = self.synthetic_unit_at(start);
        self.alloc_from(
            crate::Mark { start },
            TreeKind::Match(Match { selector, cases }),
        )
    }

    /// Parses the case region following an already parsed match selector.
    pub(super) fn parse_match_clause(&mut self, selector: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(selector)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let mark = Mark { start };

        let mut next_offset = 1;
        while matches!(
            self.cursor.lookahead(next_offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            next_offset += 1;
        }
        let next_token = self.cursor.lookahead(next_offset).clone();
        let next_kind = next_token.kind;
        let has_braced_cases = next_kind == TokenKind::Punctuation(Punctuation::LeftBrace);
        let has_inline_case = self.features().sub_cases
            && next_kind == TokenKind::Keyword(dotty_core::HardKeyword::Case)
            && !self.has_physical_line_break(self.current().span.end(), next_token.span.start());
        let case_region = if !has_braced_cases && !has_inline_case {
            // Ask the scanner to identify the active case region, whether
            // its Indent was emitted eagerly or opened by parser feedback.
            self.observe_match_cases_indented()
        } else {
            None
        };
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
                let cases = self
                    .case_clauses_in_region(case_region.map(|(indent_offset, _)| indent_offset));
                let closed_by_delimiter =
                    self.current().kind == TokenKind::Punctuation(Punctuation::RightParen);
                if closed_by_delimiter
                    && case_region.is_some_and(|(_, opened_by_feedback)| opened_by_feedback)
                {
                    self.observe_outdented_by_delimiter();
                } else if closed_by_delimiter && !self.cursor.at(TokenKind::Outdent) {
                    self.observe_outdented();
                } else if !self.cursor.at(TokenKind::Outdent)
                    && let Some((indent_offset, opened_by_feedback)) = case_region
                {
                    if opened_by_feedback {
                        self.observe_match_cases_closed(indent_offset);
                    } else {
                        self.observe_outdented_layout_region(indent_offset);
                    }
                } else if !self.cursor.at(TokenKind::Outdent) {
                    self.observe_outdented();
                }
                if !self.accept(TokenKind::Outdent) && !closed_by_delimiter {
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
    use dotty_core::ast::{Block, CaseDef, InfixOp, Match as MatchTree, Modifier, UntypedNode};
    use dotty_core::{
        HardKeyword, NameInterner, ScannerEvent, SourceId, SourceText, TextRange, Token,
        TokenSource,
    };
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FeedbackRegionKind {
        MatchCases,
        CaseBody,
    }

    struct FeedbackTokenSource {
        source: String,
        tokens: Vec<Token>,
        index: usize,
        arrow_indents: bool,
        outdent_at: Option<u32>,
        closed_feedback_by_delimiter: Rc<Cell<bool>>,
        feedback_regions: Vec<(u32, FeedbackRegionKind)>,
    }

    fn indentation_at(source: &str, offset: u32) -> usize {
        let offset = offset as usize;
        let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
        source[line_start..offset]
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count()
    }

    impl TokenSource for FeedbackTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index.min(self.tokens.len() - 1)]
        }

        fn position(&self) -> usize {
            self.index
        }

        fn advance(&mut self) {
            self.index = (self.index + 1).min(self.tokens.len() - 1);
        }

        fn lookahead(&mut self, offset: usize) -> &Token {
            &self.tokens[(self.index + offset).min(self.tokens.len() - 1)]
        }

        fn observe(&mut self, event: ScannerEvent) {
            if event == ScannerEvent::OutdentedByDelimiter {
                self.closed_feedback_by_delimiter.set(true);
                self.feedback_regions.pop();
            }
            if let ScannerEvent::OutdentedRegion { indent_offset }
            | ScannerEvent::OutdentedLayoutRegion { indent_offset } = event
                && self
                    .feedback_regions
                    .last()
                    .is_some_and(|(offset, _)| *offset == indent_offset)
                && indentation_at(&self.source, self.current().span.start())
                    < indentation_at(&self.source, indent_offset)
            {
                let offset = self.current().span.start();
                self.tokens.insert(
                    self.index,
                    Token::new(TokenKind::Outdent, TextRange::new(offset, offset).unwrap()),
                );
                self.feedback_regions.pop();
            }
            match (event, self.current().kind) {
                (ScannerEvent::MatchCasesIndented, TokenKind::Keyword(HardKeyword::Match)) => {
                    let mut indent_index = self.index + 1;
                    while matches!(
                        self.tokens[indent_index].kind,
                        TokenKind::Newline | TokenKind::Newlines
                    ) {
                        indent_index += 1;
                    }
                    let offset = self.tokens[indent_index].span.start();
                    self.tokens.insert(
                        indent_index,
                        Token::new(TokenKind::Indent, TextRange::new(offset, offset).unwrap()),
                    );
                    self.feedback_regions
                        .push((offset, FeedbackRegionKind::MatchCases));
                }
                (ScannerEvent::CaseBodyIndented { case_start }, TokenKind::Operator)
                    if self.arrow_indents
                        && self.tokens[self.index + 1..]
                            .iter()
                            .take_while(|token| {
                                matches!(token.kind, TokenKind::Newline | TokenKind::Newlines)
                            })
                            .next()
                            .is_some()
                        && self.tokens[self.index + 1..]
                            .iter()
                            .find(|token| {
                                !matches!(token.kind, TokenKind::Newline | TokenKind::Newlines)
                            })
                            .is_some_and(|token| {
                                indentation_at(&self.source, token.span.start())
                                    > indentation_at(&self.source, case_start)
                            }) =>
                {
                    let mut indent_index = self.index + 1;
                    while matches!(
                        self.tokens[indent_index].kind,
                        TokenKind::Newline | TokenKind::Newlines
                    ) {
                        indent_index += 1;
                    }
                    let offset = self.tokens[indent_index].span.start();
                    self.tokens.insert(
                        indent_index,
                        Token::new(TokenKind::Indent, TextRange::new(offset, offset).unwrap()),
                    );
                    self.feedback_regions
                        .push((offset, FeedbackRegionKind::CaseBody));
                }
                (ScannerEvent::Outdented, _)
                    if self
                        .feedback_regions
                        .last()
                        .is_some_and(|(_, kind)| *kind == FeedbackRegionKind::CaseBody)
                        && self
                            .outdent_at
                            .is_none_or(|offset| self.current().span.start() == offset) =>
                {
                    let offset = self.current().span.start();
                    self.tokens.insert(
                        self.index,
                        Token::new(TokenKind::Outdent, TextRange::new(offset, offset).unwrap()),
                    );
                    self.feedback_regions.pop();
                }
                (ScannerEvent::Outdented, _)
                    if self
                        .feedback_regions
                        .last()
                        .is_some_and(|(_, kind)| *kind == FeedbackRegionKind::MatchCases)
                        && self.tokens[self.index..]
                            .iter()
                            .find(|token| {
                                !matches!(token.kind, TokenKind::Newline | TokenKind::Newlines)
                            })
                            .is_some_and(|token| {
                                token.kind == TokenKind::Keyword(HardKeyword::Case)
                            }) => {}
                (ScannerEvent::Outdented, _)
                    if self
                        .feedback_regions
                        .last()
                        .is_some_and(|(_, kind)| *kind == FeedbackRegionKind::MatchCases) =>
                {
                    let offset = self.current().span.start();
                    self.tokens.insert(
                        self.index,
                        Token::new(TokenKind::Outdent, TextRange::new(offset, offset).unwrap()),
                    );
                    self.feedback_regions.pop();
                }
                (ScannerEvent::MatchCasesClosed { .. }, _)
                    if self
                        .feedback_regions
                        .last()
                        .is_some_and(|(_, kind)| *kind == FeedbackRegionKind::MatchCases) =>
                {
                    let offset = self.current().span.start();
                    self.tokens.insert(
                        self.index,
                        Token::new(TokenKind::Outdent, TextRange::new(offset, offset).unwrap()),
                    );
                    self.feedback_regions.pop();
                }
                _ => {}
            }
        }
    }

    #[test]
    fn requests_layout_feedback_for_match_cases_inside_braces() {
        let source = "{\n  value match\n    case A => a\n    case B => b\n  after\n}";
        let tokens = vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 4, 9),
            token(TokenKind::Keyword(HardKeyword::Match), 10, 15),
            token(TokenKind::Keyword(HardKeyword::Case), 20, 24),
            token(TokenKind::Identifier, 25, 26),
            token(TokenKind::Operator, 27, 29),
            token(TokenKind::Identifier, 30, 31),
            token(TokenKind::Newline, 31, 32),
            token(TokenKind::Keyword(HardKeyword::Case), 36, 40),
            token(TokenKind::Identifier, 41, 42),
            token(TokenKind::Operator, 43, 45),
            token(TokenKind::Identifier, 46, 47),
            token(TokenKind::Newline, 47, 48),
            token(TokenKind::Identifier, 50, 55),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 56, 57),
            token(TokenKind::Eof, 57, 57),
        ];
        let mut names = NameInterner::new();
        let mut parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            FeedbackTokenSource {
                source: source.to_owned(),
                tokens,
                index: 0,
                arrow_indents: true,
                outdent_at: None,
                closed_feedback_by_delimiter: Rc::new(Cell::new(false)),
                feedback_regions: Vec::new(),
            },
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(tree).kind else {
            panic!("expected the surrounding braced expression block");
        };
        assert_eq!(stats.len(), 1);
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(stats[0]).kind else {
            panic!("expected the match expression");
        };
        assert_eq!(
            cases.len(),
            2,
            "expected both match cases; diagnostics: {:?}",
            parser.diagnostics()
        );
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(
            parser.diagnostics().is_empty(),
            "unexpected diagnostics: {:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn nested_feedback_case_region_stops_before_the_less_indented_outer_case() {
        let source = "{ outer match\n  case A => inner match\n    case B => b\n  case D => d\n}";
        let mut offset = 0usize;
        let mut source_token = |spelling: &str, kind: TokenKind| {
            let start = source[offset..]
                .find(spelling)
                .map(|relative| offset + relative)
                .expect("token spelling occurs after the preceding token");
            let end = start + spelling.len();
            offset = end;
            token(kind, start as u32, end as u32)
        };
        let tokens = vec![
            source_token("{", TokenKind::Punctuation(Punctuation::LeftBrace)),
            source_token("outer", TokenKind::Identifier),
            source_token("match", TokenKind::Keyword(HardKeyword::Match)),
            source_token("\n", TokenKind::Newline),
            source_token("case", TokenKind::Keyword(HardKeyword::Case)),
            source_token("A", TokenKind::Identifier),
            source_token("=>", TokenKind::Operator),
            source_token("inner", TokenKind::Identifier),
            source_token("match", TokenKind::Keyword(HardKeyword::Match)),
            source_token("\n", TokenKind::Newline),
            source_token("case", TokenKind::Keyword(HardKeyword::Case)),
            source_token("B", TokenKind::Identifier),
            source_token("=>", TokenKind::Operator),
            source_token("b", TokenKind::Identifier),
            source_token("\n", TokenKind::Newline),
            source_token("case", TokenKind::Keyword(HardKeyword::Case)),
            source_token("D", TokenKind::Identifier),
            source_token("=>", TokenKind::Operator),
            source_token("d", TokenKind::Identifier),
            source_token("\n", TokenKind::Newline),
            source_token("}", TokenKind::Punctuation(Punctuation::RightBrace)),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ];
        let mut names = NameInterner::new();
        let mut parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            FeedbackTokenSource {
                source: source.to_owned(),
                tokens,
                index: 0,
                arrow_indents: true,
                outdent_at: None,
                closed_feedback_by_delimiter: Rc::new(Cell::new(false)),
                feedback_regions: Vec::new(),
            },
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Block(Block { expr, .. }) = parser.ast().get(tree).kind else {
            panic!("expected the surrounding brace block");
        };
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(expr).kind else {
            panic!("expected the outer match");
        };
        assert_eq!(cases.len(), 2);
        let TreeKind::CaseDef(CaseDef { body, .. }) = &parser.ast().get(cases[0]).kind else {
            panic!("expected the first outer case");
        };
        let TreeKind::Block(Block { expr, .. }) = parser.ast().get(*body).kind else {
            panic!("expected the outer case body block");
        };
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(expr).kind else {
            panic!("expected the nested match");
        };
        assert_eq!(cases.len(), 1);
        assert!(
            parser.diagnostics().is_empty(),
            "{:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn separates_indented_case_bodies_from_later_cases_and_following_expressions() {
        let source = "{ value match\n    case A =>\n      a\n    case B =>\n      b\n  after }";
        let tokens = vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 7),
            token(TokenKind::Keyword(HardKeyword::Match), 8, 13),
            token(TokenKind::Keyword(HardKeyword::Case), 18, 22),
            token(TokenKind::Identifier, 23, 24),
            token(TokenKind::Operator, 25, 27),
            token(TokenKind::Identifier, 34, 35),
            token(TokenKind::Keyword(HardKeyword::Case), 40, 44),
            token(TokenKind::Identifier, 45, 46),
            token(TokenKind::Operator, 47, 49),
            token(TokenKind::Identifier, 56, 57),
            token(TokenKind::Newline, 57, 58),
            token(TokenKind::Identifier, 60, 65),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 66, 67),
            token(TokenKind::Eof, 67, 67),
        ];
        let mut names = NameInterner::new();
        let mut parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            FeedbackTokenSource {
                source: source.to_owned(),
                tokens,
                index: 0,
                arrow_indents: true,
                outdent_at: None,
                closed_feedback_by_delimiter: Rc::new(Cell::new(false)),
                feedback_regions: Vec::new(),
            },
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(tree).kind else {
            panic!("expected the surrounding braced expression block");
        };
        assert_eq!(
            stats.len(),
            1,
            "expected match stat and following expression, diagnostics: {:?}",
            parser.diagnostics()
        );
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(stats[0]).kind else {
            panic!("expected the match expression");
        };
        assert_eq!(cases.len(), 2);
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(
            parser.diagnostics().is_empty(),
            "unexpected diagnostics: {:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn unindented_case_body_stops_at_the_enclosing_match_outdent() {
        let source =
            "{ value match\n    case A => val local = 1; local\n    case B => 0\n  after }";
        let tokens = vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 7),
            token(TokenKind::Keyword(HardKeyword::Match), 8, 13),
            token(TokenKind::Keyword(HardKeyword::Case), 18, 22),
            token(TokenKind::Identifier, 23, 24),
            token(TokenKind::Operator, 25, 27),
            token(TokenKind::Keyword(HardKeyword::Val), 28, 31),
            token(TokenKind::Identifier, 32, 37),
            token(TokenKind::Operator, 38, 39),
            token(TokenKind::IntegerLiteral, 40, 41),
            token(TokenKind::Punctuation(Punctuation::Semicolon), 41, 42),
            token(TokenKind::Identifier, 43, 48),
            token(TokenKind::Newline, 48, 49),
            token(TokenKind::Keyword(HardKeyword::Case), 53, 57),
            token(TokenKind::Identifier, 58, 59),
            token(TokenKind::Operator, 60, 62),
            token(TokenKind::IntegerLiteral, 63, 64),
            token(TokenKind::Newline, 64, 65),
            token(TokenKind::Identifier, 67, 72),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 73, 74),
            token(TokenKind::Eof, 74, 74),
        ];
        let mut names = NameInterner::new();
        let mut parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            FeedbackTokenSource {
                source: source.to_owned(),
                tokens,
                index: 0,
                arrow_indents: false,
                outdent_at: Some(64),
                closed_feedback_by_delimiter: Rc::new(Cell::new(false)),
                feedback_regions: Vec::new(),
            },
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(tree).kind else {
            panic!("expected the surrounding braced expression block");
        };
        assert_eq!(stats.len(), 1);
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(stats[0]).kind else {
            panic!("expected the match expression");
        };
        assert_eq!(cases.len(), 2);
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(
            parser.diagnostics().is_empty(),
            "unexpected diagnostics: {:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn sub_cases_does_not_treat_a_newline_case_as_inline_inside_braces() {
        let source = "{\n  value match\n    case A => a\n    case B => b\n}";
        let tokens = vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 4, 9),
            token(TokenKind::Keyword(HardKeyword::Match), 10, 15),
            // The scanner suppresses the physical newline and layout tokens
            // inside braces until the parser requests indentation feedback.
            token(TokenKind::Keyword(HardKeyword::Case), 20, 24),
            token(TokenKind::Identifier, 25, 26),
            token(TokenKind::Operator, 27, 29),
            token(TokenKind::Identifier, 30, 31),
            token(TokenKind::Keyword(HardKeyword::Case), 36, 40),
            token(TokenKind::Identifier, 41, 42),
            token(TokenKind::Operator, 43, 45),
            token(TokenKind::Identifier, 46, 47),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 48, 49),
            token(TokenKind::Eof, 49, 49),
        ];
        let mut names = NameInterner::new();
        let mut parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            FeedbackTokenSource {
                source: source.to_owned(),
                tokens,
                index: 0,
                arrow_indents: true,
                outdent_at: None,
                closed_feedback_by_delimiter: Rc::new(Cell::new(false)),
                feedback_regions: Vec::new(),
            },
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            sub_cases: true,
            ..crate::ParserFeatures::default()
        });

        let tree = parser.expr();

        let TreeKind::Block(Block { expr, .. }) = parser.ast().get(tree).kind else {
            panic!("expected the surrounding braced expression block");
        };
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(expr).kind else {
            panic!("expected an indented match case region");
        };
        assert_eq!(cases.len(), 2);
        assert!(
            parser.diagnostics().is_empty(),
            "{:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

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
    fn parses_statement_sequences_in_unindented_braced_case_bodies() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "value match { case A => val local = 1; local; case B => var result = 2; result }",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Keyword(HardKeyword::Match), 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Case), 14, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Keyword(HardKeyword::Val), 24, 27),
                token(TokenKind::Identifier, 28, 33),
                token(TokenKind::Operator, 34, 35),
                token(TokenKind::IntegerLiteral, 36, 37),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 37, 38),
                token(TokenKind::Identifier, 39, 44),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 44, 45),
                token(TokenKind::Keyword(HardKeyword::Case), 46, 50),
                token(TokenKind::Identifier, 51, 52),
                token(TokenKind::Operator, 53, 55),
                token(TokenKind::Keyword(HardKeyword::Var), 56, 59),
                token(TokenKind::Identifier, 60, 66),
                token(TokenKind::Operator, 67, 68),
                token(TokenKind::IntegerLiteral, 69, 70),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 70, 71),
                token(TokenKind::Identifier, 72, 78),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 79, 80),
                token(TokenKind::Eof, 80, 80),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(tree).kind else {
            panic!("expected match tree");
        };
        assert_eq!(cases.len(), 2);

        for (index, (case, expected_span)) in cases
            .iter()
            .copied()
            .zip([
                TextRange::new(21, 45).unwrap(),
                TextRange::new(53, 78).unwrap(),
            ])
            .enumerate()
        {
            let TreeKind::CaseDef(CaseDef { body, .. }) = parser.ast().get(case).kind else {
                panic!("expected case definition");
            };
            assert_eq!(
                parser.ast().get(body).position.unwrap().span().range(),
                expected_span
            );
            let TreeKind::Block(ref block) = parser.ast().get(body).kind else {
                panic!("expected block case body");
            };
            assert_eq!(block.stats.len(), 1);
            let TreeKind::ValDef(definition) = &parser.ast().get(block.stats[0]).kind else {
                panic!("expected a local value definition");
            };
            assert_eq!(
                definition.metadata.modifiers.contains(&Modifier::Var),
                index == 1,
                "case {index} should preserve its val/var distinction"
            );
            assert!(matches!(
                parser.ast().get(block.expr).kind,
                TreeKind::Ident(_)
            ));
        }

        assert!(
            parser.diagnostics().is_empty(),
            "{:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
    fn recovers_selector_match_case_without_arrow_before_following_block_statement() {
        let source = "{ value.match { case A case B => b }; after }";
        let tokens = vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Identifier, 2, 7),
            token(TokenKind::Punctuation(Punctuation::Dot), 7, 8),
            token(TokenKind::Keyword(HardKeyword::Match), 8, 13),
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 14, 15),
            token(TokenKind::Keyword(HardKeyword::Case), 16, 20),
            token(TokenKind::Identifier, 21, 22),
            token(TokenKind::Keyword(HardKeyword::Case), 23, 27),
            token(TokenKind::Identifier, 28, 29),
            token(TokenKind::Operator, 30, 32),
            token(TokenKind::Identifier, 33, 34),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 35, 36),
            token(TokenKind::Punctuation(Punctuation::Semicolon), 36, 37),
            token(TokenKind::Identifier, 38, 43),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 44, 45),
            token(TokenKind::Eof, 45, 45),
        ];
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, tokens, &mut names);

        let tree = parser.expr();

        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(tree).kind else {
            panic!("expected surrounding block");
        };
        assert_eq!(
            stats.len(),
            1,
            "stats: {stats:?}, expr: {:?}, current: {:?}, diagnostics: {:?}",
            parser.ast().get(expr).kind,
            parser.current().kind,
            parser.diagnostics()
        );
        let TreeKind::Match(MatchTree { ref cases, .. }) = parser.ast().get(stats[0]).kind else {
            panic!("expected selector match");
        };
        assert_eq!(cases.len(), 2);
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parenthesized_match_cases_stop_before_the_following_block_statement() {
        let source = "{\n  (x match\n    case A => 1\n    case _ => 2)\n  after\n}";
        let tokens = vec![
            token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
            token(TokenKind::Newline, 1, 2),
            token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
            token(TokenKind::Identifier, 5, 6),
            token(TokenKind::Keyword(HardKeyword::Match), 7, 12),
            token(TokenKind::Newline, 12, 13),
            token(TokenKind::Keyword(HardKeyword::Case), 17, 21),
            token(TokenKind::Identifier, 22, 23),
            token(TokenKind::Operator, 24, 26),
            token(TokenKind::IntegerLiteral, 27, 28),
            token(TokenKind::Newline, 28, 29),
            token(TokenKind::Keyword(HardKeyword::Case), 33, 37),
            token(TokenKind::Identifier, 38, 39),
            token(TokenKind::Operator, 40, 42),
            token(TokenKind::IntegerLiteral, 43, 44),
            token(TokenKind::Punctuation(Punctuation::RightParen), 44, 45),
            token(TokenKind::Newline, 45, 46),
            token(TokenKind::Identifier, 48, 53),
            token(TokenKind::Newline, 53, 54),
            token(TokenKind::Punctuation(Punctuation::RightBrace), 54, 55),
            token(TokenKind::Eof, 55, 55),
        ];
        let mut names = NameInterner::new();
        let closed_feedback_by_delimiter = Rc::new(Cell::new(false));
        let mut parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            FeedbackTokenSource {
                source: source.to_owned(),
                tokens,
                index: 0,
                arrow_indents: false,
                outdent_at: Some(0),
                closed_feedback_by_delimiter: Rc::clone(&closed_feedback_by_delimiter),
                feedback_regions: Vec::new(),
            },
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::Block(Block { stats, expr }) = &parser.ast().get(tree).kind else {
            panic!("expected the enclosing block expression");
        };
        assert_eq!(
            stats.len(),
            1,
            "unexpected block stats: {:?}",
            stats
                .iter()
                .map(|id| &parser.ast().get(*id).kind)
                .collect::<Vec<_>>()
        );
        let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = &parser.ast().get(stats[0]).kind
        else {
            panic!("expected the parenthesized match as the preceding statement");
        };
        assert!(matches!(
            parser.ast().get(parens.inner).kind,
            TreeKind::Match(_)
        ));
        assert!(matches!(parser.ast().get(*expr).kind, TreeKind::Ident(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(closed_feedback_by_delimiter.get());
        assert!(
            parser.diagnostics().is_empty(),
            "{:?}",
            parser.diagnostics()
        );
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
