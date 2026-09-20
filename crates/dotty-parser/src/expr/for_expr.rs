use dotty_core::ast::{ForDo, ForYield, GenAlias, GenCheckMode, GenFrom, UntypedNode};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use super::can_start_expr;
use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn parse_for_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        self.consume_for_newlines();

        let (enums, wrapped, indented) = if self.current().kind
            == TokenKind::Punctuation(Punctuation::LeftParen)
            && self.paren_starts_wrapped_enumerators()
        {
            self.advance();
            let enums =
                self.parse_enumerators(Some(TokenKind::Punctuation(Punctuation::RightParen)));
            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            (enums, true, false)
        } else if self.accept(TokenKind::Punctuation(Punctuation::LeftBrace)) {
            let enums =
                self.parse_enumerators(Some(TokenKind::Punctuation(Punctuation::RightBrace)));
            self.expect(TokenKind::Punctuation(Punctuation::RightBrace));
            (enums, true, false)
        } else {
            let indented = self.accept(TokenKind::Indent);
            (self.parse_enumerators(None), false, indented)
        };
        if indented {
            self.consume_for_newlines();
            if !self.accept(TokenKind::Outdent) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close for enumerators",
                );
            }
        }
        if wrapped {
            self.consume_for_newlines();
        }

        let kind = match self.current().kind {
            TokenKind::Keyword(HardKeyword::Yield) => {
                self.advance();
                Some(true)
            }
            TokenKind::Keyword(HardKeyword::Do) => {
                self.advance();
                Some(false)
            }
            _ => {
                if !wrapped {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected `yield` or `do` after for enumerators",
                    );
                }
                None
            }
        };

        let body = self.parse_for_body();
        if kind == Some(true) {
            self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::ForYield(ForYield { enums, body })),
            )
        } else {
            self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::ForDo(ForDo { enums, body })),
            )
        }
    }

    fn parse_enumerators(&mut self, end: Option<TokenKind>) -> Vec<TreeId<Untyped>> {
        let mut enums = Vec::new();
        let mut saw_generator = false;
        self.consume_for_separators();

        while !self.at_for_body_keyword()
            && self.current().kind != TokenKind::Eof
            && self.current().kind != TokenKind::Outdent
            && !end.is_some_and(|kind| self.current().kind == kind)
        {
            let checkpoint = self.cursor.checkpoint();
            if self.current().kind == TokenKind::Keyword(HardKeyword::If) {
                if enums.is_empty() {
                    self.report(
                        ParseDiagnosticKind::ExpectedPattern,
                        "expected a generator before a for guard",
                    );
                }
                if let Some(guard) = self.parse_guard() {
                    enums.push(guard);
                }
            } else {
                let (enumerator, generator) = self.parse_enumerator();
                enums.push(enumerator);
                saw_generator |= generator;
            }

            self.consume_for_separators();
            while self.current().kind == TokenKind::Keyword(HardKeyword::If) {
                if let Some(guard) = self.parse_guard() {
                    enums.push(guard);
                }
                self.consume_for_separators();
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing for enumerators",
                );
                break;
            }
        }

        if !saw_generator {
            self.report(
                ParseDiagnosticKind::ExpectedPattern,
                "a for comprehension requires a generator",
            );
        }
        enums
    }

    fn parse_enumerator(&mut self) -> (TreeId<Untyped>, bool) {
        if self.current().kind == TokenKind::Keyword(HardKeyword::Case) {
            self.advance();
            return (self.parse_generator(GenCheckMode::FilterAlways), true);
        }

        let mark = self.mark();
        let pattern = self.parse_pattern1_for_enumerator();
        if self.current_is_operator("<-") {
            self.advance();
            let expr = self.parse_enumerator_rhs();
            return (
                self.alloc_from(
                    mark,
                    TreeKind::PhaseSpecific(UntypedNode::GenFrom(GenFrom {
                        pattern,
                        expr,
                        check_mode: GenCheckMode::Check,
                    })),
                ),
                true,
            );
        }
        if self.current_is_operator("=") {
            self.advance();
            let expr = self.parse_enumerator_rhs();
            return (
                self.alloc_from(
                    mark,
                    TreeKind::PhaseSpecific(UntypedNode::GenAlias(GenAlias { pattern, expr })),
                ),
                false,
            );
        }

        self.report(
            ParseDiagnosticKind::ExpectedToken,
            "expected `<-` or `=` after for pattern",
        );
        if !self.at_for_body_keyword()
            && !matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Punctuation(Punctuation::Semicolon)
                    | TokenKind::Punctuation(Punctuation::RightParen)
                    | TokenKind::Punctuation(Punctuation::RightBrace)
                    | TokenKind::Outdent
                    | TokenKind::Eof
            )
        {
            self.recover_until(crate::RecoverySet::Enumerator);
        }
        (self.error_pattern(self.current_span()), false)
    }

    fn parse_generator(&mut self, check_mode: GenCheckMode) -> TreeId<Untyped> {
        let mark = self.mark();
        let pattern = self.parse_pattern1_for_enumerator();
        if !self.current_is_operator("<-") {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `<-` after case generator pattern",
            );
            return self.error_pattern(self.current_span());
        }
        self.advance();
        let expr = self.parse_enumerator_rhs();
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::GenFrom(GenFrom {
                pattern,
                expr,
                check_mode,
            })),
        )
    }

    fn parse_pattern1_for_enumerator(&mut self) -> TreeId<Untyped> {
        self.with_parse_kind(ParseKind::Pattern, |parser| {
            parser.with_location(Location::InPattern, |parser| parser.pattern1())
        })
    }

    fn parse_enumerator_rhs(&mut self) -> TreeId<Untyped> {
        self.consume_for_newlines();
        let indented = self.accept(TokenKind::Indent);
        let expr = if can_start_expr(self.current().kind) {
            self.expr()
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedExpression,
                "expected an expression after for enumerator operator",
            );
            self.error_expr(self.current_span())
        };
        if indented {
            self.consume_for_newlines();
            self.accept(TokenKind::Outdent);
        }
        expr
    }

    fn parse_for_body(&mut self) -> TreeId<Untyped> {
        self.consume_for_newlines();
        let indented = self.accept(TokenKind::Indent);
        let body = if can_start_expr(self.current().kind) {
            self.expr()
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedExpression,
                "expected an expression after for body delimiter",
            );
            self.error_expr(self.current_span())
        };
        if indented {
            self.consume_for_newlines();
            self.accept(TokenKind::Outdent);
        }
        body
    }

    fn at_for_body_keyword(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Yield | HardKeyword::Do)
        )
    }

    fn consume_for_newlines(&mut self) {
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

    fn consume_for_separators(&mut self) {
        self.consume_for_newlines();
        while self.accept(TokenKind::Punctuation(Punctuation::Semicolon)) {
            self.consume_for_newlines();
        }
    }

    fn current_is_operator(&mut self, spelling: &str) -> bool {
        self.current().kind == TokenKind::Operator && self.current_text_is(spelling)
    }

    fn paren_starts_wrapped_enumerators(&mut self) -> bool {
        let mut depth = 0usize;
        let mut offset = 0usize;
        loop {
            match self.cursor.lookahead(offset).kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => depth += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let next = self.cursor.lookahead(offset + 1);
                        return !(next.kind == TokenKind::Operator
                            && self
                                .source
                                .slice(next.span)
                                .ok()
                                .is_some_and(|text| matches!(text, "<-" | "=")));
                    }
                }
                TokenKind::Eof => return true,
                _ => {}
            }
            offset += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::UntypedNode;
    use dotty_core::{NameInterner, Token, TokenValue};

    #[test]
    fn parses_an_ordinary_generator_with_check_mode() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Keyword(HardKeyword::Yield), 12, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        assert_eq!(for_tree.enums.len(), 1);
        let TreeKind::PhaseSpecific(UntypedNode::GenFrom(generator)) =
            parser.ast().get(for_tree.enums[0]).kind
        else {
            panic!("expected GenFrom");
        };
        assert_eq!(generator.check_mode, GenCheckMode::Check);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_case_generator_with_filter_always_mode() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for case x <- xs yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Keyword(HardKeyword::Case), 4, 8),
                token(TokenKind::Identifier, 9, 10),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(11, 13).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 14, 16),
                token(TokenKind::Keyword(HardKeyword::Yield), 17, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        let TreeKind::PhaseSpecific(UntypedNode::GenFrom(generator)) =
            parser.ast().get(for_tree.enums[0]).kind
        else {
            panic!("expected GenFrom");
        };
        assert_eq!(generator.check_mode, GenCheckMode::FilterAlways);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_a_guard_after_a_generator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs if ok yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Keyword(HardKeyword::If), 12, 14),
                token(TokenKind::Identifier, 15, 17),
                token(TokenKind::Keyword(HardKeyword::Yield), 18, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        assert_eq!(for_tree.enums.len(), 2);
        assert!(matches!(
            parser.ast().get(for_tree.enums[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::GenFrom(_))
        ));
        assert!(matches!(
            parser.ast().get(for_tree.enums[1]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_alias_as_a_gen_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs; y = value yield y",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(15, 16).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 17, 22),
                token(TokenKind::Keyword(HardKeyword::Yield), 23, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        assert_eq!(for_tree.enums.len(), 2);
        assert!(matches!(
            parser.ast().get(for_tree.enums[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::GenAlias(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_parenthesized_enumerators() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for (x <- xs) yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(7, 9).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 10, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Keyword(HardKeyword::Yield), 14, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::ForYield(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_braced_for_do_with_an_application_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for { x <- xs } do consume()",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Identifier, 6, 7),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(8, 10).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 11, 13),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 14, 15),
                token(TokenKind::Keyword(HardKeyword::Do), 16, 18),
                token(TokenKind::Identifier, 19, 26),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 26, 27),
                token(TokenKind::Punctuation(Punctuation::RightParen), 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::ForDo(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_for_without_enumerators() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let _ = parser.expr();

        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_from_a_generator_without_an_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Keyword(HardKeyword::Yield), 9, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let _ = parser.expr();

        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_from_a_for_pattern_without_an_enumerator_operator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x xs yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Identifier, 6, 8),
                token(TokenKind::Keyword(HardKeyword::Yield), 9, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let _ = parser.expr();

        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_yield_visible_after_a_missing_guard_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs if yield x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Keyword(HardKeyword::If), 12, 14),
                token(TokenKind::Keyword(HardKeyword::Yield), 15, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let tree = parser.expr();

        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::ForYield(_))
        ));
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }
}
