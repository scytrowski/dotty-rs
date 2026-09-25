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
        let guard = self.parse_guard();
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
            let indented = self.accept(TokenKind::Indent);
            let body = self.with_case_body(|parser| {
                parser.with_location(Location::InBlock, |parser| parser.expr())
            });
            if indented {
                self.consume_case_newlines();
                if !self.accept(TokenKind::Outdent) {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected an outdent to close an expression-only case body",
                    );
                }
            }
            body
        } else {
            self.observe_arrow_indented();
            self.advance();
            self.parse_case_body(body_mark)
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
        let mut cases = Vec::new();
        self.consume_case_separators();
        while self.current().kind == TokenKind::Keyword(HardKeyword::Case) {
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

    fn parse_case_body(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.consume_case_newlines();
        if matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Case)
                | TokenKind::Punctuation(Punctuation::RightBrace)
                | TokenKind::Outdent
                | TokenKind::Eof
        ) {
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
            if !self.cursor.at(TokenKind::Outdent) {
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
            let expr = self.with_case_body(|parser| {
                parser.with_location(Location::InBlock, |parser| parser.expr())
            });
            if self.accept(TokenKind::Punctuation(Punctuation::Semicolon)) {
                // Dotty keeps the separator in the source-level case-body
                // block span. The next case still starts after it.
            }
            (mark, Vec::new(), expr)
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
