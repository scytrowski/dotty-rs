use dotty_core::ast::{ForDo, ForYield, GenAlias, GenCheckMode, GenFrom, UntypedNode};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use super::can_start_expr;
use crate::{ExpressionIssue, Location, ParseIssue, ParseKind, Parser};

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
                self.report_issue(ParseIssue::Expression(
                    ExpressionIssue::ExpectedForEnumeratorOutdent {
                        found: self.current().kind,
                    },
                ));
            }
        }
        if wrapped && self.for_body_keyword_follows_newlines() {
            self.consume_for_newlines();
        }

        let (kind, body_feedback) = match self.current().kind {
            TokenKind::Keyword(HardKeyword::Yield) => {
                let feedback = self.observe_indented_body();
                self.advance();
                (Some(true), feedback)
            }
            TokenKind::Keyword(HardKeyword::Do) => {
                let feedback = self.observe_indented_body();
                self.advance();
                (Some(false), feedback)
            }
            _ => {
                if !wrapped {
                    self.report_issue(ParseIssue::Expression(
                        ExpressionIssue::ExpectedForBodyKeyword {
                            found: self.current().kind,
                        },
                    ));
                }
                (None, false)
            }
        };

        let body = self.parse_for_body(body_feedback);
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
                    self.report_issue(ParseIssue::Expression(
                        ExpressionIssue::ForGuardMissingGenerator {
                            found: self.current().kind,
                        },
                    ));
                }
                if let Some(guard) = self.parse_guard() {
                    enums.push(guard);
                }
            } else {
                let (enumerator, generator) = self.parse_enumerator();
                enums.push(enumerator);
                saw_generator |= generator;
            }

            let mut had_separator = self.consume_for_separators();
            while self.current().kind == TokenKind::Keyword(HardKeyword::If) {
                if let Some(guard) = self.parse_guard() {
                    enums.push(guard);
                }
                had_separator |= self.consume_for_separators();
            }

            let has_suppressed_newline_separator = self.current_starts_multiline_for_enumerator();

            if !self.cursor.progressed_since(checkpoint) {
                self.report_issue(ParseIssue::Expression(
                    ExpressionIssue::ForEnumeratorsNoProgress {
                        found: self.current().kind,
                    },
                ));
                break;
            }
            if !had_separator
                && !has_suppressed_newline_separator
                && self.current().kind != TokenKind::Keyword(HardKeyword::If)
                && !self.at_for_body_keyword()
                && self.current().kind != TokenKind::Eof
                && self.current().kind != TokenKind::Outdent
                && !end.is_some_and(|kind| self.current().kind == kind)
            {
                break;
            }
        }

        if !saw_generator {
            self.report_issue(ParseIssue::Expression(
                ExpressionIssue::ForMissingGenerator {
                    found: self.current().kind,
                },
            ));
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

        self.report_issue(ParseIssue::Expression(
            ExpressionIssue::ExpectedForEnumeratorOperator {
                found: self.current().kind,
            },
        ));
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
            self.report_issue(ParseIssue::Expression(
                ExpressionIssue::ExpectedCaseGeneratorOperator {
                    found: self.current().kind,
                },
            ));
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
            self.parse_for_enumerator_expression()
        } else {
            self.report_issue(ParseIssue::Expression(
                ExpressionIssue::ExpectedForEnumeratorExpression {
                    found: self.current().kind,
                },
            ));
            self.error_expr(self.current_span())
        };
        if indented {
            self.consume_for_newlines();
            self.accept(TokenKind::Outdent);
        }
        expr
    }

    fn parse_for_enumerator_expression(&mut self) -> TreeId<Untyped> {
        let previous = std::mem::replace(&mut self.for_enumerator_rhs, true);
        let expression = self.expr();
        self.for_enumerator_rhs = previous;
        expression
    }

    pub(super) fn current_starts_multiline_for_enumerator(&mut self) -> bool {
        if !matches!(
            self.last_real_token_kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::QuoteId
                | TokenKind::CharLiteral
                | TokenKind::IntegerLiteral
                | TokenKind::DecimalLiteral
                | TokenKind::ExponentLiteral
                | TokenKind::LongLiteral
                | TokenKind::FloatLiteral
                | TokenKind::DoubleLiteral
                | TokenKind::StringLiteral
                | TokenKind::StringPart
                | TokenKind::Keyword(
                    HardKeyword::Null
                        | HardKeyword::True
                        | HardKeyword::False
                        | HardKeyword::This
                        | HardKeyword::Super
                )
                | TokenKind::Punctuation(
                    Punctuation::RightParen | Punctuation::RightBracket | Punctuation::RightBrace
                )
        ) {
            return false;
        }

        let mut pattern_offset = 0;
        while matches!(
            self.cursor.lookahead(pattern_offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            pattern_offset += 1;
        }
        let pattern = self.cursor.lookahead(pattern_offset).clone();
        if !self.has_physical_line_break(self.last_real_token_end, pattern.span.start()) {
            return false;
        }
        if pattern.kind == TokenKind::Keyword(HardKeyword::Case) {
            pattern_offset += 1;
        }
        if !matches!(
            self.cursor.lookahead(pattern_offset).kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return false;
        }

        let operator = self.cursor.lookahead(pattern_offset + 1);
        operator.kind == TokenKind::Operator
            && self
                .source
                .slice(operator.span)
                .ok()
                .is_some_and(|spelling| matches!(spelling, "<-" | "="))
    }

    fn parse_for_body(&mut self, feedback_opened: bool) -> TreeId<Untyped> {
        let feedback_opened = feedback_opened
            || (matches!(
                self.current().kind,
                TokenKind::Newline | TokenKind::Newlines
            ) && self.observe_indented_body());
        self.consume_for_newlines();
        if self.current().kind == TokenKind::Indent {
            return if feedback_opened {
                self.parse_feedback_indented_block()
            } else {
                self.parse_indented_block()
            };
        }

        if can_start_expr(self.current().kind) {
            self.expr()
        } else {
            self.report_issue(ParseIssue::Expression(
                ExpressionIssue::ExpectedForBodyExpression {
                    found: self.current().kind,
                },
            ));
            self.error_expr(self.current_span())
        }
    }

    fn at_for_body_keyword(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Yield | HardKeyword::Do)
        )
    }

    fn for_body_keyword_follows_newlines(&mut self) -> bool {
        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        matches!(
            self.cursor.lookahead(offset).kind,
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

    fn consume_for_separators(&mut self) -> bool {
        let mut consumed = false;
        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            consumed = true;
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
        while self.accept(TokenKind::Punctuation(Punctuation::Semicolon)) {
            consumed = true;
            self.consume_for_newlines();
        }
        consumed
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
    use crate::ParseDiagnosticKind;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::UntypedNode;
    use dotty_core::{NameInterner, Token, TokenValue};

    fn arrow(start: u32, end: u32) -> Token {
        Token {
            kind: TokenKind::Operator,
            span: dotty_core::TextRange::new(start, end).unwrap(),
            value: TokenValue::None,
        }
    }

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
    fn physical_newline_separates_generators_when_outer_parentheses_suppress_tokens() {
        let source = "for x <- xs\n    _ <- transform(x) yield y";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                arrow(6, 8),
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Identifier, 16, 17),
                arrow(18, 20),
                token(TokenKind::Identifier, 21, 30),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 30, 31),
                token(TokenKind::Identifier, 31, 32),
                token(TokenKind::Punctuation(Punctuation::RightParen), 32, 33),
                token(TokenKind::Keyword(HardKeyword::Yield), 34, 39),
                token(TokenKind::Identifier, 40, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::PhaseSpecific(UntypedNode::ForYield(for_tree)) = &parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        assert_eq!(for_tree.enums.len(), 2);
        assert!(for_tree.enums.iter().all(|enumerator| matches!(
            parser.ast().get(*enumerator).kind,
            TreeKind::PhaseSpecific(UntypedNode::GenFrom(_))
        )));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(
            parser.ast().get(tree).position.unwrap().span().range(),
            dotty_core::TextRange::new(0, source.len() as u32).unwrap()
        );
    }

    #[test]
    fn recognizes_an_enumerator_after_a_line_break_token_in_a_lambda_rhs() {
        let source = "incremented\nresult <- values";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Identifier, 12, 18),
                arrow(19, 21),
                token(TokenKind::Identifier, 22, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );
        parser.last_real_token_kind = TokenKind::Identifier;
        parser.last_real_token_end = 11;
        parser.for_enumerator_rhs = true;

        assert!(parser.current_starts_multiline_for_enumerator());
    }

    #[test]
    fn recovers_at_yield_after_a_multiline_generator_with_missing_rhs() {
        let source = "for x <- xs\n    y <- yield y";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                arrow(6, 8),
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Identifier, 16, 17),
                arrow(18, 20),
                token(TokenKind::Keyword(HardKeyword::Yield), 21, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::PhaseSpecific(UntypedNode::ForYield(for_tree)) = &parser.ast().get(tree).kind
        else {
            panic!("expected recoverable ForYield");
        };
        assert_eq!(for_tree.enums.len(), 2);
        assert!(matches!(
            parser.ast().get(for_tree.body).kind,
            TreeKind::Ident(_)
        ));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::ExpectedExpression })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_a_second_generator_without_a_separator() {
        let source = "for x <- xs y <- ys yield y";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                arrow(6, 8),
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Identifier, 12, 13),
                arrow(14, 16),
                token(TokenKind::Identifier, 17, 19),
                token(TokenKind::Keyword(HardKeyword::Yield), 20, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let tree = parser.expr();

        let TreeKind::PhaseSpecific(UntypedNode::ForDo(for_do)) = &parser.ast().get(tree).kind
        else {
            panic!("expected recoverable ForDo");
        };
        assert_eq!(for_do.enums.len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::ExpectedToken })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
    fn parses_yield_after_a_newline_following_parenthesized_enumerators() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for (x <- xs)\nyield x",
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
                token(TokenKind::Newline, 13, 14),
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
    fn parses_parenthesized_for_body_with_indented_local_definitions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for (x <- xs)\n  def result = x\n  result\nafter",
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
                token(TokenKind::Newline, 13, 14),
                token(TokenKind::Indent, 16, 16),
                token(TokenKind::Keyword(HardKeyword::Def), 16, 19),
                token(TokenKind::Identifier, 20, 26),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(27, 28).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Newline, 30, 31),
                token(TokenKind::Identifier, 33, 39),
                token(TokenKind::Newline, 39, 40),
                token(TokenKind::Outdent, 40, 40),
                token(TokenKind::Identifier, 40, 45),
                token(TokenKind::Eof, 45, 45),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForDo(ref for_tree)) = parser.ast().get(tree).kind
        else {
            panic!("expected a for-do tree");
        };
        let TreeKind::Block(ref body) = parser.ast().get(for_tree.body).kind else {
            panic!("expected an indented statement-sequence body");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(body.stats[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(matches!(
            parser.ast().get(body.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Identifier);
        assert_eq!(parser.source.slice(parser.current().span).unwrap(), "after");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_yield_after_a_newline_following_braced_enumerators() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for { x <- xs }\nyield x",
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
                token(TokenKind::Newline, 15, 16),
                token(TokenKind::Keyword(HardKeyword::Yield), 16, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Eof, 23, 23),
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
    fn parses_indented_for_do_body_as_a_statement_sequence() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs do\n  var access = x\n  step(access)\nafter",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Keyword(HardKeyword::Do), 12, 14),
                token(TokenKind::Newline, 14, 15),
                token(TokenKind::Indent, 15, 15),
                token(TokenKind::Keyword(HardKeyword::Var), 17, 20),
                token(TokenKind::Identifier, 21, 27),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(28, 29).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 30, 31),
                token(TokenKind::Newline, 31, 32),
                token(TokenKind::Identifier, 34, 38),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 38, 39),
                token(TokenKind::Identifier, 39, 45),
                token(TokenKind::Punctuation(Punctuation::RightParen), 45, 46),
                token(TokenKind::Newline, 46, 47),
                token(TokenKind::Outdent, 47, 47),
                token(TokenKind::Identifier, 47, 52),
                token(TokenKind::Eof, 52, 52),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForDo(ref for_tree)) = parser.ast().get(tree).kind
        else {
            panic!("expected ForDo");
        };
        let TreeKind::Block(ref block) = parser.ast().get(for_tree.body).kind else {
            panic!("expected an indented statement-sequence block");
        };
        assert_eq!(block.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(block.expr).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Identifier);
        assert_eq!(parser.source.slice(parser.current().span).unwrap(), "after");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_indented_for_yield_body_as_a_statement_sequence() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs yield\n  val result = x\n  transform(result)",
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
                token(TokenKind::Newline, 17, 18),
                token(TokenKind::Indent, 18, 18),
                token(TokenKind::Keyword(HardKeyword::Val), 20, 23),
                token(TokenKind::Identifier, 24, 30),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(31, 32).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 33, 34),
                token(TokenKind::Newline, 34, 35),
                token(TokenKind::Identifier, 37, 46),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 46, 47),
                token(TokenKind::Identifier, 47, 53),
                token(TokenKind::Punctuation(Punctuation::RightParen), 53, 54),
                token(TokenKind::Outdent, 54, 54),
                token(TokenKind::Eof, 54, 54),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        let TreeKind::Block(ref block) = parser.ast().get(for_tree.body).kind else {
            panic!("expected an indented statement-sequence block");
        };
        assert_eq!(block.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(block.expr).kind,
            TreeKind::Apply(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_missing_outdent_after_an_indented_for_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "for x <- xs do\n  val result = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::For), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(6, 8).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Keyword(HardKeyword::Do), 12, 14),
                token(TokenKind::Newline, 14, 15),
                token(TokenKind::Indent, 15, 15),
                token(TokenKind::Keyword(HardKeyword::Val), 17, 20),
                token(TokenKind::Identifier, 21, 27),
                Token {
                    kind: TokenKind::Operator,
                    span: dotty_core::TextRange::new(28, 29).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let _ = parser.expr();

        let diagnostic = parser
            .diagnostics()
            .last()
            .expect("missing outdent diagnostic");
        assert_eq!(
            diagnostic.issue(),
            &ParseIssue::Expression(ExpressionIssue::ExpectedIndentedBlockOutdent {
                found: TokenKind::Eof,
            })
        );
        assert_eq!(
            diagnostic.span(),
            dotty_core::TextRange::new(31, 31).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
