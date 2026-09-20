use dotty_core::ast::{Block, If, While};
use dotty_core::{Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use super::is_else_separator;
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

    fn parse_control_body(&mut self) -> TreeId<Untyped> {
        self.consume_control_newlines();
        if self.current().kind == TokenKind::Indent {
            return self.parse_indented_block();
        }
        self.expr()
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
