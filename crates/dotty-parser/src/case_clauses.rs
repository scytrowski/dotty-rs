use dotty_core::ast::{Block, CaseDef};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseDiagnosticKind, ParseKind, Parser, RecoverySet};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses one `case Pattern [if Guard] =>` production.
    pub(crate) fn case_clause(&mut self, expr_only: bool) -> TreeId<Untyped> {
        let mark = self.mark();
        if !self.accept(TokenKind::Keyword(HardKeyword::Case)) {
            self.report(ParseDiagnosticKind::ExpectedToken, "expected `case`");
        }

        let pattern = self.with_parse_kind(ParseKind::Pattern, |parser| {
            parser.with_location(Location::InPattern, |parser| parser.pattern())
        });
        let guard = self.parse_case_guard();
        let body_mark = self.mark();

        if !self.current_is_arrow() {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` after case pattern",
            );
            self.recover_until(RecoverySet::Case);
            let body = self.error_expr(self.current_span());
            return self.alloc_from(
                mark,
                TreeKind::CaseDef(CaseDef {
                    pattern,
                    guard,
                    body,
                }),
            );
        }

        let body = if expr_only {
            self.advance();
            self.consume_case_newlines();
            self.with_case_body(|parser| {
                if parser.current().kind == TokenKind::Indent {
                    // Dotty's expression-only case production still parses an
                    // indented expression as a BlockExpr. Keep the indentation
                    // token visible so the shared case-body parser consumes
                    // the complete statement sequence and its matching outdent.
                    let body_indent = (parser.current().kind == TokenKind::Indent)
                        .then(|| parser.current().span.start());
                    let body = parser.parse_case_body(body_mark, body_indent);
                    if let TreeKind::Block(Block { stats, expr }) = &parser.ast.get(body).kind
                        && stats.is_empty()
                    {
                        *expr
                    } else {
                        body
                    }
                } else {
                    parser.with_location(Location::InBlock, |parser| parser.expr())
                }
            })
        } else {
            let body_indent_offset = self.observe_case_body_indented(mark.start);
            self.advance();
            self.parse_case_body(body_mark, body_indent_offset)
        };
        self.alloc_from(
            mark,
            TreeKind::CaseDef(CaseDef {
                pattern,
                guard,
                body,
            }),
        )
    }

    /// Parses a consecutive case list, leaving its enclosing `}`/`Outdent` untouched.
    pub(crate) fn case_clauses(&mut self) -> Vec<TreeId<Untyped>> {
        self.case_clauses_in_region(None)
    }

    /// Parses cases while allowing the scanner to end a parser- or scanner-opened case
    /// region before a less-indented outer `case` clause.
    pub(crate) fn case_clauses_in_region(
        &mut self,
        region_indent_offset: Option<u32>,
    ) -> Vec<TreeId<Untyped>> {
        let mut cases = Vec::new();
        self.consume_case_separators();
        loop {
            if self.current().kind == TokenKind::Keyword(HardKeyword::Case)
                && let Some(indent_offset) = region_indent_offset
            {
                self.observe_outdented_region(indent_offset);
            }
            if self.current().kind != TokenKind::Keyword(HardKeyword::Case) {
                break;
            }
            let checkpoint = self.cursor.checkpoint();
            cases.push(self.case_clause(false));
            self.consume_case_separators();
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing case clauses",
                );
                break;
            }
        }
        cases
    }

    /// Parses the shared `if PostfixExpr` guard production.
    pub(crate) fn parse_guard(&mut self) -> Option<TreeId<Untyped>> {
        if self.current().kind != TokenKind::Keyword(HardKeyword::If) {
            return None;
        }
        self.advance();
        if !crate::expr::can_start_prefix_expr(self.current().kind) {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedExpression,
                "expected an expression after guard `if`",
            );
            return Some(self.error_expr(position));
        }
        Some(self.with_location(Location::InGuard, |parser| parser.postfix_expr()))
    }

    /// Parses a case guard after the optional separator between its pattern
    /// and `if`. Dotty's `InCase` separator region permits this line break,
    /// but the separator is only part of the guard production when `if`
    /// immediately follows it; other case/body boundaries remain significant.
    fn parse_case_guard(&mut self) -> Option<TreeId<Untyped>> {
        if matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) && self.cursor.lookahead(1).kind == TokenKind::Keyword(HardKeyword::If)
        {
            self.advance();
        }

        self.parse_guard()
    }

    fn parse_case_body(
        &mut self,
        mark: crate::Mark,
        body_indent_offset: Option<u32>,
    ) -> TreeId<Untyped> {
        self.consume_case_newlines();
        if matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Case)
                | TokenKind::Punctuation(Punctuation::RightBrace)
                | TokenKind::Outdent
                | TokenKind::Eof
        ) {
            // Dotty accepts an empty case statement sequence; the shared AST
            // represents it as an empty block with the usual synthetic Unit.
            let expr = self.synthetic_unit();
            return self.alloc_from(
                mark,
                TreeKind::Block(Block {
                    stats: Vec::new(),
                    expr,
                }),
            );
        }

        let (block_mark, stats, expr) = if self.current().kind == TokenKind::Indent {
            self.advance();
            let result = self
                .with_case_body(|parser| parser.parse_expression_block_body(TokenKind::Outdent));
            if !self.cursor.at(TokenKind::Outdent)
                && let Some(indent_offset) = body_indent_offset
            {
                self.observe_outdented_region(indent_offset);
            } else if !self.cursor.at(TokenKind::Outdent) {
                self.observe_outdented();
            }
            if !self.accept(TokenKind::Outdent) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close a case body",
                );
            }
            let block_mark = result
                .0
                .first()
                .and_then(|stat| {
                    self.ast.get(*stat).position.map(|position| crate::Mark {
                        start: position.span().range().start(),
                    })
                })
                .unwrap_or(mark);
            (block_mark, result.0, result.1)
        } else {
            let (stats, expr) = self
                .with_case_body(|parser| parser.parse_expression_block_body(TokenKind::Outdent));
            (mark, stats, expr)
        };

        self.alloc_from(block_mark, TreeKind::Block(Block { stats, expr }))
    }

    fn consume_case_newlines(&mut self) {
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

    fn consume_case_separators(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Punctuation(Punctuation::Semicolon)
        ) {
            self.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{CaseDef, Ident, UntypedNode};
    use dotty_core::{NameInterner, ScannerEvent, SourceId, SourceText, TextRange, Token};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct RecordingTokenSource {
        tokens: Vec<Token>,
        index: usize,
        observed: Rc<RefCell<Vec<ScannerEvent>>>,
    }

    impl dotty_core::TokenSource for RecordingTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index]
        }

        fn position(&self) -> usize {
            self.index
        }

        fn advance(&mut self) {
            if self.index + 1 < self.tokens.len() {
                self.index += 1;
            }
        }

        fn lookahead(&mut self, n: usize) -> &Token {
            let index = self
                .index
                .saturating_add(n)
                .min(self.tokens.len().saturating_sub(1));
            &self.tokens[index]
        }

        fn observe(&mut self, event: ScannerEvent) {
            self.observed.borrow_mut().push(event);
        }
    }

    #[test]
    fn parses_a_case_with_a_block_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x => body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.case_clause(false);
        let TreeKind::CaseDef(CaseDef {
            pattern,
            body,
            guard,
        }) = parser.ast().get(id).kind
        else {
            panic!("expected case definition");
        };
        assert!(guard.is_none());
        assert!(matches!(
            parser.ast().get(pattern).kind,
            TreeKind::Ident(Ident { .. })
        ));
        let TreeKind::Block(ref block) = parser.ast().get(body).kind else {
            panic!("expected case body block");
        };
        assert!(block.stats.is_empty());
        assert!(matches!(
            parser.ast().get(block.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 14).unwrap()
        );
        assert_eq!(
            parser.ast().get(body).position.unwrap().span().range(),
            TextRange::new(7, 14).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_multiline_expression_only_case_without_opening_a_layout_body() {
        let mut names = NameInterner::new();
        let observed = Rc::new(RefCell::new(Vec::new()));
        let mut parser = Parser::new(
            SourceText::new("case x =>\n  body").expect("valid source"),
            SourceId::from_index(1),
            RecordingTokenSource {
                tokens: vec![
                    token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                    token(TokenKind::Identifier, 5, 6),
                    token(TokenKind::Operator, 7, 9),
                    token(TokenKind::Newline, 9, 10),
                    token(TokenKind::Indent, 12, 12),
                    token(TokenKind::Identifier, 12, 16),
                    token(TokenKind::Outdent, 16, 16),
                    token(TokenKind::Eof, 16, 16),
                ],
                index: 0,
                observed: Rc::clone(&observed),
            },
            &mut names,
        );

        let id = parser.case_clause(true);
        let TreeKind::CaseDef(CaseDef { body, .. }) = parser.ast().get(id).kind else {
            panic!("expected case definition");
        };

        assert!(matches!(parser.ast().get(body).kind, TreeKind::Ident(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(observed.borrow().is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn expression_only_case_with_an_indent_parses_the_whole_block_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x =>\n  first\n  second",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Newline, 9, 10),
                token(TokenKind::Indent, 12, 12),
                token(TokenKind::Identifier, 12, 17),
                token(TokenKind::Newline, 17, 18),
                token(TokenKind::Identifier, 20, 26),
                token(TokenKind::Outdent, 26, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let case = parser.case_clause(true);
        let TreeKind::CaseDef(case) = &parser.ast().get(case).kind else {
            panic!("expected a case clause");
        };
        let TreeKind::Block(body) = &parser.ast().get(case.body).kind else {
            panic!("an indented expression-only case body should retain its block");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(body.stats[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(body.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn indented_case_body_span_starts_at_its_first_statement() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x =>\n  body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Newline, 9, 10),
                token(TokenKind::Indent, 12, 12),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Outdent, 16, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let case = parser.case_clause(false);
        let TreeKind::CaseDef(CaseDef { body, .. }) = parser.ast().get(case).kind else {
            panic!("expected case definition");
        };
        assert_eq!(
            parser.ast().get(body).position.unwrap().span().range(),
            TextRange::new(7, 16).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_case_guard_at_the_postfix_expression_boundary() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x if x > 0 => body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Keyword(HardKeyword::If), 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Operator, 12, 13),
                token(TokenKind::IntegerLiteral, 14, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Identifier, 19, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.case_clause(false);
        let TreeKind::CaseDef(CaseDef { guard, .. }) = parser.ast().get(id).kind else {
            panic!("expected case definition");
        };
        let guard = guard.expect("expected guard");
        assert!(matches!(
            parser.ast().get(guard).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_case_guard_after_a_line_separator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x\n  if x > 0 => body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Newline, 6, 9),
                token(TokenKind::Keyword(HardKeyword::If), 9, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Operator, 14, 15),
                token(TokenKind::IntegerLiteral, 16, 17),
                token(TokenKind::Operator, 18, 20),
                token(TokenKind::Identifier, 21, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let case = parser.case_clause(false);
        let TreeKind::CaseDef(CaseDef { guard, .. }) = parser.ast().get(case).kind else {
            panic!("expected case definition");
        };
        let guard = guard.expect("expected guard");
        assert!(matches!(
            parser.ast().get(guard).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_case_guard_after_a_blank_line_separator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x\n\n  if x > 0 => body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Newlines, 6, 10),
                token(TokenKind::Keyword(HardKeyword::If), 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::IntegerLiteral, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let case = parser.case_clause(false);
        let TreeKind::CaseDef(CaseDef { guard, .. }) = parser.ast().get(case).kind else {
            panic!("expected case definition");
        };

        assert!(guard.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn leaves_a_case_separator_before_the_next_case_unconsumed() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x\ncase y => body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Newline, 6, 7),
                token(TokenKind::Keyword(HardKeyword::Case), 7, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let _ = parser.case_clause(false);

        assert_eq!(parser.current().kind, TokenKind::Newline);
    }

    #[test]
    fn recovers_from_a_missing_expression_in_a_next_line_guard() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x\n  if => body\ncase y => next",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Newline, 6, 9),
                token(TokenKind::Keyword(HardKeyword::If), 9, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Keyword(HardKeyword::Case), 20, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Operator, 27, 29),
                token(TokenKind::Identifier, 30, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        );

        let cases = parser.case_clauses();

        assert_eq!(cases.len(), 2);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedExpression)
        );
    }

    #[test]
    fn parses_multiple_case_clauses_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case A => a\ncase B => b",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Keyword(HardKeyword::Case), 12, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let cases = parser.case_clauses();

        assert_eq!(cases.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_empty_case_body_before_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case A => case B => b",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Keyword(HardKeyword::Case), 10, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let cases = parser.case_clauses();

        assert_eq!(cases.len(), 2);
        let TreeKind::CaseDef(first) = parser.ast().get(cases[0]).kind else {
            panic!("expected first case definition");
        };
        let TreeKind::Block(ref block) = parser.ast().get(first.body).kind else {
            panic!("expected empty case body block");
        };
        assert!(block.stats.is_empty());
        assert!(matches!(
            parser.ast().get(block.expr).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit
            })
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_case_without_an_arrow_before_the_next_case() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case x body\ncase y => result",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Identifier, 7, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Keyword(HardKeyword::Case), 12, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let cases = parser.case_clauses();

        assert_eq!(cases.len(), 2);
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_from_a_case_with_a_missing_pattern_without_swallowing_the_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "case => result",
            vec![
                token(TokenKind::Keyword(HardKeyword::Case), 0, 4),
                token(TokenKind::Operator, 5, 7),
                token(TokenKind::Identifier, 8, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let case = parser.case_clause(false);

        assert!(matches!(parser.ast().get(case).kind, TreeKind::CaseDef(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }
}
