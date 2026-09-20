use dotty_core::ast::{Block, If, Match, ParsedTry, Return, Throw, UntypedNode, While};
use dotty_core::{Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use super::{can_start_expr, is_else_separator};
use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn parse_if_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let parenthesized_condition =
            self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen);
        let cond = self.parse_control_condition(dotty_core::HardKeyword::Then);
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Then) {
            self.advance();
        } else if !parenthesized_condition {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected `then` after if condition",
            );
        }
        let then_branch = self.parse_control_body();
        let else_branch = if let Some(separator_end) = self.accept_else_after_optional_separator() {
            if let Some(separator_end) = separator_end {
                self.extend_tree_end(then_branch, separator_end);
            }
            self.parse_control_body()
        } else {
            self.synthetic_unit_at(self.last_real_token_end)
        };

        self.alloc_from(
            mark,
            TreeKind::If(If {
                cond,
                then_branch,
                else_branch,
            }),
        )
    }

    fn accept_else_after_optional_separator(&mut self) -> Option<Option<u32>> {
        let mut lookahead = 0;
        let mut separator_end = None;
        loop {
            let token = self.cursor.lookahead(lookahead);
            let kind = token.kind;
            if is_else_separator(kind) {
                if kind == TokenKind::Punctuation(Punctuation::Semicolon) {
                    separator_end = Some(token.span.end());
                }
                lookahead += 1;
                continue;
            }
            if kind != TokenKind::Keyword(dotty_core::HardKeyword::Else) {
                return None;
            }
            break;
        }

        while is_else_separator(self.current().kind) {
            self.advance();
        }
        self.advance();
        Some(separator_end)
    }

    fn extend_tree_end(&mut self, tree: TreeId<Untyped>, end: u32) {
        let Some(position) = self.ast.get(tree).position else {
            return;
        };
        let range = position.span().range();
        let range = TextRange::new(range.start(), end).expect("tree span endpoints are ordered");
        self.ast.get_mut(tree).position =
            Some(SourceSpan::new(self.source_id, Span::without_point(range)));
    }

    pub(super) fn parse_while_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let parenthesized_condition =
            self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen);
        let cond = self.parse_control_condition(dotty_core::HardKeyword::Do);
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Do) {
            self.advance();
        } else if !parenthesized_condition {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected `do` after while condition",
            );
        }
        let body = self.parse_control_body();

        self.alloc_from(mark, TreeKind::While(While { cond, body }))
    }

    pub(super) fn parse_throw_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let expr = self.parse_layout_expression("expected an expression after `throw`");
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Throw(Throw { expr })),
        )
    }

    pub(super) fn parse_try_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let expr = self.parse_layout_expression("expected an expression after `try`");

        let handler = if self.accept_layout_keyword(dotty_core::HardKeyword::Catch) {
            if self.catch_starts_case_handler() {
                Some(self.parse_catch_case_handler())
            } else {
                Some(self.parse_layout_expression("expected an expression after `catch`"))
            }
        } else {
            None
        };

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
                expr,
                handler,
                finalizer: None,
            })),
        )
    }

    pub(super) fn parse_return_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let indented = self.accept_layout_indent();
        let expr = can_start_expr(self.current().kind).then(|| self.expr());
        self.close_layout_expression(indented);
        self.alloc_from(mark, TreeKind::Return(Return { expr, from: None }))
    }

    fn parse_control_body(&mut self) -> TreeId<Untyped> {
        self.consume_control_newlines();
        if self.current().kind == TokenKind::Indent {
            return self.parse_indented_block();
        }
        self.expr()
    }

    fn parse_layout_expression(&mut self, message: &str) -> TreeId<Untyped> {
        let indented = self.accept_layout_indent();
        let expression = if can_start_expr(self.current().kind) {
            self.expr()
        } else {
            let position = self.current_span();
            self.report(crate::ParseDiagnosticKind::ExpectedExpression, message);
            self.error_expr(position)
        };
        self.close_layout_expression(indented);
        expression
    }

    fn accept_layout_indent(&mut self) -> bool {
        let mut lookahead = 0;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }
        if self.cursor.lookahead(lookahead).kind != TokenKind::Indent {
            return false;
        }
        self.consume_control_newlines();
        self.accept(TokenKind::Indent)
    }

    fn close_layout_expression(&mut self, indented: bool) {
        if indented {
            self.consume_control_newlines();
            if !self.accept(TokenKind::Outdent) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close an indented expression",
                );
            }
        }
    }

    fn accept_layout_keyword(&mut self, keyword: dotty_core::HardKeyword) -> bool {
        if self.current().kind == TokenKind::Keyword(keyword) {
            self.advance();
            return true;
        }

        let mut lookahead = 0;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }
        if lookahead == 0 || self.cursor.lookahead(lookahead).kind != TokenKind::Keyword(keyword) {
            return false;
        }

        self.consume_control_newlines();
        self.advance();
        true
    }

    fn catch_starts_case_handler(&mut self) -> bool {
        let mut lookahead = 0;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }

        if self.cursor.lookahead(lookahead).kind
            == TokenKind::Keyword(dotty_core::HardKeyword::Case)
        {
            return true;
        }

        if matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Indent | TokenKind::Punctuation(Punctuation::LeftBrace)
        ) {
            lookahead += 1;
            while matches!(
                self.cursor.lookahead(lookahead).kind,
                TokenKind::Newline | TokenKind::Newlines
            ) {
                lookahead += 1;
            }
            return self.cursor.lookahead(lookahead).kind
                == TokenKind::Keyword(dotty_core::HardKeyword::Case);
        }

        false
    }

    fn parse_catch_case_handler(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        self.consume_control_newlines();
        let braced = self.accept(TokenKind::Punctuation(Punctuation::LeftBrace));
        let indented = if braced {
            false
        } else {
            self.consume_control_newlines();
            self.accept(TokenKind::Indent)
        };

        let cases = if braced || indented {
            self.case_clauses()
        } else {
            vec![self.case_clause(true)]
        };

        if braced {
            if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `}` to close catch cases",
                );
            }
        } else if indented {
            self.consume_control_newlines();
            if !self.accept(TokenKind::Outdent) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close catch cases",
                );
            }
        }

        let selector = self.synthetic_unit_at(mark.start());
        self.alloc_from(mark, TreeKind::Match(Match { selector, cases }))
    }

    fn consume_control_newlines(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while consuming control-flow newlines",
                );
                break;
            }
        }
    }

    fn parse_indented_block(&mut self) -> TreeId<Untyped> {
        self.advance();
        let mark = self.mark();
        let (stats, expr) = self.parse_expression_block_body(TokenKind::Outdent);
        if !self.accept(TokenKind::Outdent) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected an outdent to close an indented block",
            );
        }
        let is_single_expression = stats.is_empty()
            && self.ast.get(expr).position.is_some_and(|position| {
                let range = position.span().range();
                range.start() != range.end()
            });
        if is_single_expression {
            expr
        } else {
            self.alloc_from(mark, TreeKind::Block(Block { stats, expr }))
        }
    }

    pub(super) fn parse_control_condition(
        &mut self,
        terminator: dotty_core::HardKeyword,
    ) -> TreeId<Untyped> {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return self.expr();
        }

        let tree = self.simple_expr();
        if self.current().kind == TokenKind::Keyword(terminator) {
            return tree;
        }

        if matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::ColonOp
        ) {
            return self.infix_expr(tree);
        }

        tree
    }
}

#[cfg(test)]
mod tests {
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Match, ParsedTry, Return, Throw, UntypedNode};
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TokenKind, TreeKind};

    #[test]
    fn parses_throw_with_a_full_application_operand() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "throw makeError()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Throw), 0, 5),
                token(TokenKind::Identifier, 6, 15),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Throw(Throw { expr })) = parser.ast().get(id).kind
        else {
            panic!("expected throw tree");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Apply(_)));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            dotty_core::TextRange::new(0, 17).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_throw_with_a_control_flow_operand() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "throw if c then yes else no",
            vec![
                token(TokenKind::Keyword(HardKeyword::Throw), 0, 5),
                token(TokenKind::Keyword(HardKeyword::If), 6, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Keyword(HardKeyword::Then), 11, 15),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::Keyword(HardKeyword::Else), 20, 24),
                token(TokenKind::Identifier, 25, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Throw(Throw { expr })) = parser.ast().get(id).kind
        else {
            panic!("expected throw tree");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::If(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_throw_with_an_indented_operand() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "throw\n  makeError()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Throw), 0, 5),
                token(TokenKind::Indent, 8, 8),
                token(TokenKind::Identifier, 8, 17),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Outdent, 19, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Throw(Throw { expr })) = parser.ast().get(id).kind
        else {
            panic!("expected throw tree");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Apply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_throw_without_an_operand() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "throw",
            vec![
                token(TokenKind::Keyword(HardKeyword::Throw), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Throw(Throw { expr })) = parser.ast().get(id).kind
        else {
            panic!("expected throw tree");
        };

        assert!(matches!(
            parser.ast().get(expr).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_try_body_as_a_source_level_parsed_try() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            expr,
            handler,
            finalizer,
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Apply(_)));
        assert!(handler.is_none());
        assert!(finalizer.is_none());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            dotty_core::TextRange::new(0, 11).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_try_body_with_an_indented_layout_region() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try\n  risky()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Indent, 6, 6),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Outdent, 13, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry { expr, .. })) =
            parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Apply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_try_without_a_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry { expr, .. })) =
            parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree");
        };

        assert!(matches!(
            parser.ast().get(expr).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_try_with_an_expression_catch_handler() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch recover()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Identifier, 18, 25),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 25, 26),
                token(TokenKind::Punctuation(Punctuation::RightParen), 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            expr,
            handler: Some(handler),
            finalizer,
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with a handler");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Apply(_)));
        assert!(matches!(parser.ast().get(handler).kind, TreeKind::Apply(_)));
        assert!(finalizer.is_none());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_try_with_an_indented_expression_catch_handler() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch\n  recover()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Indent, 20, 20),
                token(TokenKind::Identifier, 20, 27),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 27, 28),
                token(TokenKind::Punctuation(Punctuation::RightParen), 28, 29),
                token(TokenKind::Outdent, 29, 29),
                token(TokenKind::Eof, 29, 29),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: Some(handler),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with an indented handler");
        };

        assert!(matches!(parser.ast().get(handler).kind, TreeKind::Apply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_try_with_a_single_case_catch_handler() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch case error => recover(error)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Keyword(HardKeyword::Case), 18, 22),
                token(TokenKind::Identifier, 23, 28),
                token(TokenKind::Operator, 29, 31),
                token(TokenKind::Identifier, 32, 39),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 39, 40),
                token(TokenKind::Identifier, 40, 45),
                token(TokenKind::Punctuation(Punctuation::RightParen), 45, 46),
                token(TokenKind::Eof, 46, 46),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: Some(handler),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with a case handler");
        };

        let TreeKind::Match(Match {
            selector,
            ref cases,
        }) = parser.ast().get(handler).kind
        else {
            panic!("expected selectorless match handler");
        };
        assert!(parser.ast().get(selector).position.is_some_and(|position| {
            let range = position.span().range();
            range.start() == range.end()
        }));
        assert_eq!(cases.len(), 1);
        assert!(matches!(
            parser.ast().get(cases[0]).kind,
            TreeKind::CaseDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_try_with_braced_catch_cases() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch { case error => recover(error) }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 18, 19),
                token(TokenKind::Keyword(HardKeyword::Case), 20, 24),
                token(TokenKind::Identifier, 25, 30),
                token(TokenKind::Operator, 31, 33),
                token(TokenKind::Identifier, 34, 41),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 41, 42),
                token(TokenKind::Identifier, 42, 47),
                token(TokenKind::Punctuation(Punctuation::RightParen), 47, 48),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 49, 50),
                token(TokenKind::Eof, 50, 50),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: Some(handler),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with braced cases");
        };

        let TreeKind::Match(Match { ref cases, .. }) = parser.ast().get(handler).kind else {
            panic!("expected match handler");
        };
        assert_eq!(cases.len(), 1);
        assert!(matches!(
            parser.ast().get(cases[0]).kind,
            TreeKind::CaseDef(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_bare_return_without_an_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "return",
            vec![
                token(TokenKind::Keyword(HardKeyword::Return), 0, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::Return(Return { expr, from }) = parser.ast().get(id).kind else {
            panic!("expected return tree");
        };

        assert!(expr.is_none());
        assert!(from.is_none());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_return_with_a_full_expression_operand() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "return value + 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Return), 0, 6),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Operator, 13, 14),
                token(TokenKind::IntegerLiteral, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::Return(Return { expr, from }) = parser.ast().get(id).kind else {
            panic!("expected return tree");
        };
        let expr = expr.expect("expected return expression");

        assert!(matches!(
            parser.ast().get(expr).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(from.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_return_with_an_indented_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "return\n  value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Return), 0, 6),
                token(TokenKind::Newline, 6, 7),
                token(TokenKind::Indent, 9, 9),
                token(TokenKind::Identifier, 9, 14),
                token(TokenKind::Outdent, 14, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::Return(Return { expr, .. }) = parser.ast().get(id).kind else {
            panic!("expected return tree");
        };

        assert!(matches!(
            parser
                .ast()
                .get(expr.expect("expected return expression"))
                .kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn does_not_consume_else_as_a_bare_return_operand() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "return else",
            vec![
                token(TokenKind::Keyword(HardKeyword::Return), 0, 6),
                token(TokenKind::Keyword(HardKeyword::Else), 7, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::Return(Return { expr, .. }) = parser.ast().get(id).kind else {
            panic!("expected return tree");
        };

        assert!(expr.is_none());
        assert_eq!(parser.current().kind, TokenKind::Keyword(HardKeyword::Else));
        assert!(parser.diagnostics().is_empty());
    }
}
