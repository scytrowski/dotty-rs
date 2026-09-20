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

        let indented = self.accept(TokenKind::Indent);
        let enums = self.parse_enumerators();
        if indented {
            self.consume_for_newlines();
            if !self.accept(TokenKind::Outdent) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close for enumerators",
                );
            }
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
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `yield` or `do` after for enumerators",
                );
                None
            }
        };

        let body = self.parse_for_body();
        if kind == Some(true) {
            self.alloc_from(mark, TreeKind::PhaseSpecific(UntypedNode::ForYield(ForYield {
                enums,
                body,
            })))
        } else {
            self.alloc_from(mark, TreeKind::PhaseSpecific(UntypedNode::ForDo(ForDo {
                enums,
                body,
            })))
        }
    }

    fn parse_enumerators(&mut self) -> Vec<TreeId<Untyped>> {
        let mut enums = Vec::new();
        let mut saw_generator = false;
        self.consume_for_separators();

        while !self.at_for_body_keyword()
            && self.current().kind != TokenKind::Eof
            && self.current().kind != TokenKind::Outdent
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
                    | TokenKind::Outdent
                    | TokenKind::Eof
            )
        {
            self.advance();
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
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) = parser.ast().get(tree).kind
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
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) = parser.ast().get(tree).kind
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
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) = parser.ast().get(tree).kind
        else {
            panic!("expected ForYield");
        };
        assert_eq!(for_tree.enums.len(), 2);
        assert!(matches!(
            parser.ast().get(for_tree.enums[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::GenFrom(_))
        ));
        assert!(matches!(parser.ast().get(for_tree.enums[1]).kind, TreeKind::Ident(_)));
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
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(ref for_tree)) = parser.ast().get(tree).kind
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
}
