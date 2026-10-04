use dotty_core::ast::{Block, If, Match, ParsedTry, Return, Throw, UntypedNode, While};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use super::{can_start_expr, is_else_separator};
use crate::{Location, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn parse_if_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let condition_feedback = self.observe_indented_body();
        self.advance();
        let parenthesized_condition =
            self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen);
        let cond = self.parse_control_condition(dotty_core::HardKeyword::Then, condition_feedback);
        let then_body_feedback =
            if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Then) {
                let feedback = self.observe_indented_body_region_from(mark.start).is_some();
                self.advance();
                feedback
            } else if !parenthesized_condition {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `then` after if condition",
                );
                false
            } else {
                false
            };
        let then_branch = self.parse_control_body(then_body_feedback);
        let else_branch = if let Some((separator_end, body_feedback)) =
            self.accept_else_after_optional_separator(mark.start)
        {
            if let Some(separator_end) = separator_end {
                self.extend_tree_end(then_branch, separator_end);
            }
            self.parse_control_body(body_feedback)
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

    fn accept_else_after_optional_separator(
        &mut self,
        if_start: u32,
    ) -> Option<(Option<u32>, bool)> {
        let mut lookahead = 0;
        let mut separator_end = None;
        loop {
            let token = self.cursor.lookahead(lookahead).clone();
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
            let if_indent = self.source_line_indent_prefix(if_start);
            let else_indent = self.source_line_indent_prefix(token.span.start());
            if self.context.block_end != Some(TokenKind::Punctuation(Punctuation::RightBrace))
                && else_indent.len() < if_indent.len()
                && if_indent.starts_with(&else_indent)
            {
                return None;
            }
            break;
        }

        while is_else_separator(self.current().kind) {
            self.advance();
        }
        let body_feedback = self.observe_indented_body();
        self.advance();
        Some((separator_end, body_feedback))
    }

    pub(super) fn parse_while_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let condition_feedback = self.observe_indented_body();
        self.advance();
        let parenthesized_condition =
            self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen);
        let cond = self.parse_control_condition(dotty_core::HardKeyword::Do, condition_feedback);
        let body_feedback =
            if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Do) {
                let feedback = self.observe_indented_body();
                self.advance();
                feedback
            } else if !parenthesized_condition {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `do` after while condition",
                );
                false
            } else {
                false
            };
        let body = self.parse_control_body(body_feedback);

        self.alloc_from(mark, TreeKind::While(While { cond, body }))
    }

    pub(super) fn parse_do_while_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let body_feedback = self.observe_indented_body();
        self.advance();

        let body = if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::While)
            || self.current().kind == TokenKind::Eof
        {
            self.report(
                crate::ParseDiagnosticKind::ExpectedExpression,
                "expected a body after `do`",
            );
            self.error_expr(self.current_span())
        } else {
            self.parse_control_body(body_feedback)
        };

        let mut newline_count = 0;
        while matches!(
            self.cursor.lookahead(newline_count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            newline_count += 1;
        }
        if newline_count > 0
            && self.cursor.lookahead(newline_count).kind
                == TokenKind::Keyword(dotty_core::HardKeyword::While)
        {
            self.consume_control_newlines();
        }

        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::While) {
            self.advance();
        } else {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected `while` after `do` body",
            );
        }

        let condition = if crate::expr::can_start_expr(self.current().kind)
            && !matches!(
                self.current().kind,
                TokenKind::Newline | TokenKind::Newlines | TokenKind::Outdent | TokenKind::Eof
            ) {
            self.expr()
        } else {
            self.report(
                crate::ParseDiagnosticKind::ExpectedExpression,
                "expected a condition after `while`",
            );
            self.error_expr(self.current_span())
        };

        let body_start = self
            .ast
            .get(body)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or(mark.start());
        let condition_block = self.alloc_from(
            crate::Mark { start: body_start },
            TreeKind::Block(Block {
                stats: vec![body],
                expr: condition,
            }),
        );
        let unit_body = self.synthetic_unit_at(self.last_real_token_end);

        self.alloc_from(
            mark,
            TreeKind::While(While {
                cond: condition_block,
                body: unit_body,
            }),
        )
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
        let body_feedback = self.observe_indented_body();
        self.advance();
        let expr = self.parse_try_body(body_feedback, "expected an expression after `try`");

        let handler = if let Some(feedback_opened) = self.accept_catch_keyword() {
            if self.catch_starts_case_handler() {
                Some(self.parse_catch_case_handler(feedback_opened))
            } else {
                Some(self.parse_layout_expression("expected an expression after `catch`"))
            }
        } else {
            None
        };
        if handler.is_some_and(|handler| self.is_empty_block(handler)) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedExpression,
                "catch handler cannot be empty",
            );
        }
        let finalizer = self
            .accept_layout_keyword(dotty_core::HardKeyword::Finally)
            .map(|feedback_opened| {
                self.parse_try_body(feedback_opened, "expected an expression after `finally`")
            });

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
                expr,
                handler,
                finalizer,
            })),
        )
    }

    fn parse_try_body(&mut self, feedback_opened: bool, message: &str) -> TreeId<Untyped> {
        let mut lookahead = 0;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }

        if self.cursor.lookahead(lookahead).kind == TokenKind::Indent {
            self.consume_control_newlines();
            if feedback_opened {
                self.parse_feedback_indented_block()
            } else {
                self.parse_indented_block()
            }
        } else {
            self.parse_layout_expression(message)
        }
    }

    pub(super) fn parse_return_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let indented = self.accept_layout_indent();
        let expr = can_start_expr(self.current().kind).then(|| self.expr());
        self.close_layout_expression(indented);
        self.alloc_from(mark, TreeKind::Return(Return { expr, from: None }))
    }

    fn parse_control_body(&mut self, feedback_opened: bool) -> TreeId<Untyped> {
        let feedback_opened = feedback_opened
            || (matches!(
                self.current().kind,
                TokenKind::Newline | TokenKind::Newlines
            ) && self.observe_indented_body());
        self.consume_control_newlines();
        if self.current().kind == TokenKind::Indent {
            return if feedback_opened {
                self.parse_feedback_indented_block()
            } else {
                self.parse_indented_block()
            };
        }
        if !can_start_expr(self.current().kind) {
            let position = self.current_span();
            self.report(
                crate::ParseDiagnosticKind::ExpectedExpression,
                "expected an expression for the control-flow branch",
            );
            return self.error_expr(position);
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

    fn accept_layout_keyword(&mut self, keyword: dotty_core::HardKeyword) -> Option<bool> {
        if self.current().kind != TokenKind::Keyword(keyword) {
            let mut lookahead = 0;
            while matches!(
                self.cursor.lookahead(lookahead).kind,
                TokenKind::Newline | TokenKind::Newlines
            ) {
                lookahead += 1;
            }
            if lookahead == 0
                || self.cursor.lookahead(lookahead).kind != TokenKind::Keyword(keyword)
            {
                return None;
            }
            self.consume_control_newlines();
        }

        let feedback_opened = self.observe_indented_body();
        self.advance();
        Some(feedback_opened)
    }

    /// Consumes `catch`, opening a parser-requested case region when a
    /// multiline handler appears inside a braced scope. The ordinary scanner
    /// layout pass suppresses that region there, but Dotty still parses the
    /// handler as a case list rather than as one expression-only case.
    fn accept_catch_keyword(&mut self) -> Option<Option<(u32, bool)>> {
        let mut keyword_offset = 0;
        while matches!(
            self.cursor.lookahead(keyword_offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            keyword_offset += 1;
        }
        if self.cursor.lookahead(keyword_offset).kind
            != TokenKind::Keyword(dotty_core::HardKeyword::Catch)
        {
            return None;
        }

        self.consume_control_newlines();
        let case_handler = self.catch_keyword_is_followed_by_cases();
        let first_body_token = self.first_catch_handler_token();
        let requests_feedback_region = case_handler
            && self.catch_has_multiple_unindented_cases()
            && first_body_token.kind != TokenKind::Punctuation(Punctuation::LeftBrace)
            && first_body_token.kind != TokenKind::Indent
            && self
                .has_physical_line_break(self.current().span.end(), first_body_token.span.start());
        let case_region = if requests_feedback_region {
            self.observe_match_cases_indented()
        } else if first_body_token.kind == TokenKind::Indent {
            Some((first_body_token.span.start(), false))
        } else {
            None
        };

        self.advance();
        Some(case_region)
    }

    fn catch_has_multiple_unindented_cases(&mut self) -> bool {
        let mut offset = 1;
        let mut delimiter_depth = 0u32;
        let mut layout_depth = 0u32;
        let mut first_case_column = None;
        let line_index = self.source.line_index().ok();

        loop {
            let token = self.cursor.lookahead(offset).clone();
            match token.kind {
                TokenKind::Eof => return false,
                TokenKind::Punctuation(Punctuation::RightBrace) if delimiter_depth == 0 => {
                    return false;
                }
                TokenKind::Indent => layout_depth += 1,
                TokenKind::Outdent => layout_depth = layout_depth.saturating_sub(1),
                TokenKind::Punctuation(
                    Punctuation::LeftParen | Punctuation::LeftBracket | Punctuation::LeftBrace,
                ) => delimiter_depth += 1,
                TokenKind::Punctuation(
                    Punctuation::RightParen | Punctuation::RightBracket | Punctuation::RightBrace,
                ) => delimiter_depth = delimiter_depth.saturating_sub(1),
                TokenKind::Keyword(dotty_core::HardKeyword::Case)
                    if delimiter_depth == 0 && layout_depth == 0 =>
                {
                    if let Some(line_index) = &line_index {
                        let column = line_index
                            .utf8_column(self.source.as_str(), token.span.start())
                            .ok();
                        if first_case_column.is_some_and(|first| column == Some(first)) {
                            return true;
                        }
                        first_case_column.get_or_insert(column.unwrap_or_default());
                    }
                }
                _ => {}
            }
            offset += 1;
        }
    }

    fn first_catch_handler_token(&mut self) -> dotty_core::Token {
        let mut offset = 1;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        self.cursor.lookahead(offset).clone()
    }

    fn catch_keyword_is_followed_by_cases(&mut self) -> bool {
        let first = self.first_catch_handler_token();
        match first.kind {
            TokenKind::Keyword(dotty_core::HardKeyword::Case) => true,
            TokenKind::Indent | TokenKind::Punctuation(Punctuation::LeftBrace) => {
                let mut offset = 1;
                while matches!(
                    self.cursor.lookahead(offset).kind,
                    TokenKind::Newline | TokenKind::Newlines
                ) {
                    offset += 1;
                }
                if matches!(
                    self.cursor.lookahead(offset).kind,
                    TokenKind::Indent | TokenKind::Punctuation(Punctuation::LeftBrace)
                ) {
                    offset += 1;
                }
                while matches!(
                    self.cursor.lookahead(offset).kind,
                    TokenKind::Newline | TokenKind::Newlines
                ) {
                    offset += 1;
                }
                self.cursor.lookahead(offset).kind
                    == TokenKind::Keyword(dotty_core::HardKeyword::Case)
            }
            _ => false,
        }
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
            return matches!(
                self.cursor.lookahead(lookahead).kind,
                TokenKind::Keyword(dotty_core::HardKeyword::Case)
            );
        }

        false
    }

    fn parse_catch_case_handler(&mut self, case_region: Option<(u32, bool)>) -> TreeId<Untyped> {
        let initial_mark = self.mark();
        self.consume_control_newlines();
        let braced = self.accept(TokenKind::Punctuation(Punctuation::LeftBrace));
        let indented = if braced {
            false
        } else {
            self.consume_control_newlines();
            self.accept(TokenKind::Indent)
        };
        let mark = if braced { initial_mark } else { self.mark() };

        let cases = if braced {
            self.case_clauses()
        } else if indented {
            self.case_clauses_in_region(case_region.map(|(indent_offset, _)| indent_offset))
        } else {
            vec![self.case_clause(true)]
        };
        if cases.is_empty() {
            self.report(
                crate::ParseDiagnosticKind::ExpectedPattern,
                "expected at least one `case` clause after `catch`",
            );
        }

        if braced {
            if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `}` to close catch cases",
                );
            }
        } else if indented {
            self.consume_control_newlines();
            if !self.cursor.at(TokenKind::Outdent)
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

    fn is_empty_block(&self, tree: TreeId<Untyped>) -> bool {
        let TreeKind::Block(Block { stats, expr }) = &self.ast.get(tree).kind else {
            return false;
        };
        stats.is_empty()
            && self.ast.get(*expr).position.is_some_and(|position| {
                let range = position.span().range();
                range.start() == range.end()
            })
    }

    pub(crate) fn consume_control_newlines(&mut self) {
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

    pub(crate) fn parse_indented_block(&mut self) -> TreeId<Untyped> {
        self.parse_indented_block_with_feedback(None)
    }

    pub(crate) fn parse_feedback_indented_block(&mut self) -> TreeId<Untyped> {
        self.parse_indented_block_with_feedback(Some(self.current().span.start()))
    }

    pub(crate) fn parse_region_feedback_indented_block(
        &mut self,
        indent_offset: u32,
    ) -> TreeId<Untyped> {
        self.parse_indented_block_with_feedback(Some(indent_offset))
    }

    fn parse_indented_block_with_feedback(
        &mut self,
        feedback_indent: Option<u32>,
    ) -> TreeId<Untyped> {
        self.advance();
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Case) {
            let case_mark = self.mark();
            let cases = self.case_clauses();
            let closed_by_delimiter = feedback_indent.is_some()
                && matches!(
                    self.current().kind,
                    TokenKind::Punctuation(Punctuation::RightParen | Punctuation::RightBrace)
                )
                || (feedback_indent.is_some()
                    && self.context.location == Location::InArgs
                    && self.current().kind == TokenKind::Punctuation(Punctuation::Comma));
            if closed_by_delimiter {
                self.observe_outdented_by_delimiter();
            } else if let Some(indent_offset) = feedback_indent {
                if !self.cursor.at(TokenKind::Outdent) {
                    self.observe_outdented_region(indent_offset);
                }
            } else if !self.cursor.at(TokenKind::Outdent) {
                self.observe_outdented();
            }
            if !closed_by_delimiter && !self.accept(TokenKind::Outdent) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected an outdent to close case-lambda clauses",
                );
            }
            let selector = self.synthetic_unit_at(case_mark.start);
            return self.alloc_from(case_mark, TreeKind::Match(Match { selector, cases }));
        }

        let mark = self.mark();
        let (stats, expr) = if let Some(indent_offset) = feedback_indent {
            self.parse_region_feedback_expression_block_body(indent_offset)
        } else {
            self.parse_expression_block_body(TokenKind::Outdent)
        };
        let closed_by_delimiter = feedback_indent.is_some()
            && matches!(
                self.current().kind,
                TokenKind::Punctuation(Punctuation::RightParen | Punctuation::RightBrace)
            )
            || (feedback_indent.is_some()
                && self.context.location == Location::InArgs
                && self.current().kind == TokenKind::Punctuation(Punctuation::Comma));
        if closed_by_delimiter {
            self.observe_outdented_by_delimiter();
        } else if let Some(indent_offset) = feedback_indent {
            self.observe_outdented_region(indent_offset);
        }
        if !closed_by_delimiter && !self.accept(TokenKind::Outdent) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected an outdent to close an indented block",
            );
        }
        self.finish_indented_block(mark, stats, expr)
    }

    fn finish_indented_block(
        &mut self,
        mark: crate::Mark,
        stats: Vec<TreeId<Untyped>>,
        expr: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
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
        feedback_opened: bool,
    ) -> TreeId<Untyped> {
        if feedback_opened || self.control_condition_starts_with_indent() {
            self.consume_control_newlines();
            if self.current().kind == TokenKind::Indent {
                return if feedback_opened {
                    self.parse_feedback_indented_block()
                } else {
                    self.parse_indented_block()
                };
            }
        }

        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return self.expr();
        }

        // Dotty continues a parenthesized condition when the next token cannot
        // start a statement, or when the matching `then`/`do` is found ahead.
        // This matters for block arguments anywhere in the suffix chain, not
        // only when `{` immediately follows `)`.
        let continues_condition = self.parenthesized_condition_should_continue(terminator);
        let tree = if continues_condition {
            self.simple_expr()
        } else {
            self.parenthesized_condition_atom()
        };
        if self.current().kind == TokenKind::Keyword(terminator) {
            return tree;
        }

        if continues_condition
            && matches!(
                self.current().kind,
                TokenKind::Operator | TokenKind::ColonOp
            )
        {
            return self.infix_expr(tree);
        }

        tree
    }

    fn control_condition_starts_with_indent(&mut self) -> bool {
        let mut lookahead = 0;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }
        self.cursor.lookahead(lookahead).kind == TokenKind::Indent
    }

    fn parenthesized_condition_should_continue(
        &mut self,
        terminator: dotty_core::HardKeyword,
    ) -> bool {
        let mut paren_depth = 0usize;
        let mut offset = 0usize;
        let after_condition = loop {
            let kind = self.cursor.lookahead(offset).kind;
            match kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => paren_depth += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    let Some(depth) = paren_depth.checked_sub(1) else {
                        return false;
                    };
                    paren_depth = depth;
                    if paren_depth == 0 {
                        break offset + 1;
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset += 1;
        };

        let next_kind = self.cursor.lookahead(after_condition).kind;
        if matches!(next_kind, TokenKind::Newline | TokenKind::Newlines) {
            return false;
        }
        let next = self.cursor.lookahead(after_condition).clone();
        if next.kind == TokenKind::Punctuation(Punctuation::Dot)
            || matches!(
                next.kind,
                TokenKind::ColonOp | TokenKind::ColonFollow | TokenKind::ColonEol
            )
            || (next.kind == TokenKind::Operator
                && !matches!(self.token_text(&next).ok(), Some("-" | "+" | "~" | "!")))
        {
            return true;
        }

        let mut delimiters = Vec::new();
        offset = after_condition;
        loop {
            let kind = self.cursor.lookahead(offset).kind;
            if kind == TokenKind::XmlStart {
                return false;
            }
            if delimiters.is_empty() {
                if kind == TokenKind::Keyword(terminator) {
                    return true;
                }
                if is_control_lookahead_boundary(kind) {
                    return false;
                }
            }

            match kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => {
                    delimiters.push(Punctuation::RightParen)
                }
                TokenKind::Punctuation(Punctuation::LeftBracket) => {
                    delimiters.push(Punctuation::RightBracket)
                }
                TokenKind::Punctuation(Punctuation::LeftBrace) => {
                    delimiters.push(Punctuation::RightBrace)
                }
                TokenKind::Punctuation(
                    close @ (Punctuation::RightParen
                    | Punctuation::RightBracket
                    | Punctuation::RightBrace),
                ) => {
                    if delimiters.last() == Some(&close) {
                        delimiters.pop();
                    } else {
                        return false;
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset += 1;
        }
    }
}

const fn is_control_lookahead_boundary(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Outdent
            | TokenKind::Eof
            | TokenKind::Punctuation(Punctuation::Semicolon)
            | TokenKind::Keyword(
                dotty_core::HardKeyword::If
                    | dotty_core::HardKeyword::Else
                    | dotty_core::HardKeyword::While
                    | dotty_core::HardKeyword::Do
                    | dotty_core::HardKeyword::For
                    | dotty_core::HardKeyword::Yield
                    | dotty_core::HardKeyword::New
                    | dotty_core::HardKeyword::Try
                    | dotty_core::HardKeyword::Catch
                    | dotty_core::HardKeyword::Finally
                    | dotty_core::HardKeyword::Throw
                    | dotty_core::HardKeyword::Return
                    | dotty_core::HardKeyword::Match
                    | dotty_core::HardKeyword::Val
                    | dotty_core::HardKeyword::Var
                    | dotty_core::HardKeyword::Def
                    | dotty_core::HardKeyword::Type
                    | dotty_core::HardKeyword::Class
                    | dotty_core::HardKeyword::Trait
                    | dotty_core::HardKeyword::Object
                    | dotty_core::HardKeyword::Enum
                    | dotty_core::HardKeyword::Given
                    | dotty_core::HardKeyword::Import
                    | dotty_core::HardKeyword::Export
                    | dotty_core::HardKeyword::Package
                    | dotty_core::HardKeyword::Abstract
                    | dotty_core::HardKeyword::Final
                    | dotty_core::HardKeyword::Sealed
                    | dotty_core::HardKeyword::Implicit
                    | dotty_core::HardKeyword::Lazy
                    | dotty_core::HardKeyword::Private
                    | dotty_core::HardKeyword::Protected
                    | dotty_core::HardKeyword::Override
            )
            | TokenKind::CaseClass
            | TokenKind::CaseObject
    )
}

#[cfg(test)]
mod tests {
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Block, Match, ParsedTry, Return, Throw, UntypedNode};
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TokenKind, TreeKind};

    #[test]
    fn dedented_else_belongs_to_the_outer_if() {
        let source = "if x then\n  if y then 1\nelse if z then 3\nelse 4";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Keyword(HardKeyword::Then), 5, 9),
                token(TokenKind::Newline, 9, 10),
                token(TokenKind::Keyword(HardKeyword::If), 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Keyword(HardKeyword::Then), 17, 21),
                token(TokenKind::IntegerLiteral, 22, 23),
                token(TokenKind::Newline, 23, 24),
                token(TokenKind::Keyword(HardKeyword::Else), 24, 28),
                token(TokenKind::Keyword(HardKeyword::If), 29, 31),
                token(TokenKind::Identifier, 32, 33),
                token(TokenKind::Keyword(HardKeyword::Then), 34, 38),
                token(TokenKind::IntegerLiteral, 39, 40),
                token(TokenKind::Newline, 40, 41),
                token(TokenKind::Keyword(HardKeyword::Else), 41, 45),
                token(TokenKind::IntegerLiteral, 46, 47),
                token(TokenKind::Eof, 47, 47),
            ],
            &mut names,
        );

        let expression = parser.expr();
        let TreeKind::If(outer) = &parser.ast().get(expression).kind else {
            panic!("expected an outer if expression");
        };
        assert!(matches!(
            parser.ast().get(outer.then_branch).kind,
            TreeKind::If(_)
        ));
        assert!(matches!(
            parser.ast().get(outer.else_branch).kind,
            TreeKind::If(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_parenthesized_legacy_if_branches_out_of_the_condition_application() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (cond) (yes) else no",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Keyword(HardKeyword::Else), 16, 20),
                token(TokenKind::Identifier, 21, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.cond).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(matches!(
            parser.ast().get(if_tree.else_branch).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_a_prefix_expression_branch_out_of_a_parenthesized_if_condition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (cond) -1 else no",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::IntegerLiteral, 11, 12),
                token(TokenKind::Keyword(HardKeyword::Else), 13, 17),
                token(TokenKind::Identifier, 18, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.cond).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_a_parenthesized_legacy_while_body_out_of_the_condition_application() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "while (cond) (step)",
            vec![
                token(TokenKind::Keyword(HardKeyword::While), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 13, 14),
                token(TokenKind::Identifier, 14, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
            panic!("expected while tree");
        };

        assert!(matches!(
            parser.ast().get(while_tree.cond).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(matches!(
            parser.ast().get(while_tree.body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_braced_legacy_if_branches_after_parenthesized_condition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (c) { a } else { b }",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Keyword(HardKeyword::Else), 13, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.cond).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::Block(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.else_branch).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_else_after_a_newline_following_a_braced_legacy_branch() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (c) { a }\nelse { b }",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Newline, 12, 13),
                token(TokenKind::Keyword(HardKeyword::Else), 13, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::Block(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.else_branch).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_braced_legacy_while_body_after_parenthesized_condition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "while (c) { step }",
            vec![
                token(TokenKind::Keyword(HardKeyword::While), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 10, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
            panic!("expected while tree");
        };

        assert!(matches!(
            parser.ast().get(while_tree.body).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_braced_argument_in_a_parenthesized_if_condition_before_then() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (f) { arg } then yes else no",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Identifier, 9, 12),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 13, 14),
                token(TokenKind::Keyword(HardKeyword::Then), 15, 19),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Keyword(HardKeyword::Else), 24, 28),
                token(TokenKind::Identifier, 29, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.cond).kind,
            TreeKind::Apply(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.else_branch).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_braced_argument_in_a_parenthesized_while_condition_before_do() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "while (f) { arg } do body",
            vec![
                token(TokenKind::Keyword(HardKeyword::While), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Keyword(HardKeyword::Do), 18, 20),
                token(TokenKind::Identifier, 21, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::While(while_tree) = parser.ast().get(id).kind else {
            panic!("expected while tree");
        };

        assert!(matches!(
            parser.ast().get(while_tree.cond).kind,
            TreeKind::Apply(_)
        ));
        assert!(matches!(
            parser.ast().get(while_tree.body).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_braced_argument_after_a_selection_suffix_in_parenthesized_if_condition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (f).check { arg } then yes else no",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::Dot), 6, 7),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 13, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Then), 21, 25),
                token(TokenKind::Identifier, 26, 29),
                token(TokenKind::Keyword(HardKeyword::Else), 30, 34),
                token(TokenKind::Identifier, 35, 37),
                token(TokenKind::Eof, 37, 37),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.cond).kind,
            TreeKind::Apply(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.else_branch).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_braced_argument_after_an_application_suffix_in_parenthesized_if_condition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "if (f)(x) { arg } then yes else no",
            vec![
                token(TokenKind::Keyword(HardKeyword::If), 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Keyword(HardKeyword::Then), 18, 22),
                token(TokenKind::Identifier, 23, 26),
                token(TokenKind::Keyword(HardKeyword::Else), 27, 31),
                token(TokenKind::Identifier, 32, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::If(if_tree) = parser.ast().get(id).kind else {
            panic!("expected if tree");
        };

        assert!(matches!(
            parser.ast().get(if_tree.cond).kind,
            TreeKind::Apply(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.then_branch).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(if_tree.else_branch).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

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
    fn parses_a_try_body_with_local_definitions_as_an_expression_block() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try\n  val value = 1\n  value\ncatch recover()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Indent, 6, 6),
                token(TokenKind::Keyword(HardKeyword::Val), 6, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::IntegerLiteral, 18, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Identifier, 22, 27),
                token(TokenKind::Outdent, 27, 27),
                token(TokenKind::Keyword(HardKeyword::Catch), 28, 33),
                token(TokenKind::Identifier, 34, 41),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 41, 42),
                token(TokenKind::Punctuation(Punctuation::RightParen), 42, 43),
                token(TokenKind::Eof, 43, 43),
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
        let try_body = expr;
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(try_body).kind else {
            panic!("expected the indented try body to remain a block");
        };

        assert_eq!(stats.len(), 1);
        assert!(matches!(
            parser.ast().get(stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.ast().get(try_body).position.unwrap().span().range(),
            dotty_core::TextRange::new(6, 27).unwrap()
        );
        assert!(matches!(parser.ast().get(handler).kind, TreeKind::Apply(_)));
        assert!(finalizer.is_none());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_multiple_try_body_definitions_before_the_final_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try\n  val first = 1\n  val second = first + 1\n  second\ncatch recover()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Indent, 6, 6),
                token(TokenKind::Keyword(HardKeyword::Val), 6, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::IntegerLiteral, 18, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Keyword(HardKeyword::Val), 22, 25),
                token(TokenKind::Identifier, 26, 32),
                token(TokenKind::Operator, 33, 34),
                token(TokenKind::Identifier, 35, 40),
                token(TokenKind::Operator, 41, 42),
                token(TokenKind::IntegerLiteral, 43, 44),
                token(TokenKind::Newline, 44, 45),
                token(TokenKind::Identifier, 47, 53),
                token(TokenKind::Outdent, 53, 53),
                token(TokenKind::Keyword(HardKeyword::Catch), 54, 59),
                token(TokenKind::Identifier, 60, 67),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 67, 68),
                token(TokenKind::Punctuation(Punctuation::RightParen), 68, 69),
                token(TokenKind::Eof, 69, 69),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            expr,
            handler: Some(_),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with a handler");
        };
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(expr).kind else {
            panic!("expected an expression block for the try body");
        };

        assert_eq!(stats.len(), 2);
        assert!(
            stats
                .iter()
                .all(|stat| matches!(parser.ast().get(*stat).kind, TreeKind::ValDef(_)))
        );
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_finally_outside_an_indented_try_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try\n  val value = 1\n  value\nfinally cleanup()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Indent, 6, 6),
                token(TokenKind::Keyword(HardKeyword::Val), 6, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::IntegerLiteral, 18, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Identifier, 22, 27),
                token(TokenKind::Outdent, 27, 27),
                token(TokenKind::Keyword(HardKeyword::Finally), 28, 35),
                token(TokenKind::Identifier, 36, 43),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 43, 44),
                token(TokenKind::Punctuation(Punctuation::RightParen), 44, 45),
                token(TokenKind::Eof, 45, 45),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            expr,
            handler,
            finalizer: Some(finalizer),
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with a finalizer");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Block(_)));
        assert!(handler.is_none());
        assert!(matches!(
            parser.ast().get(finalizer).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_when_an_indented_try_body_reaches_eof_without_an_outdent() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try\n  val value = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Indent, 6, 6),
                token(TokenKind::Keyword(HardKeyword::Val), 6, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::IntegerLiteral, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry { expr, .. })) =
            parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree");
        };

        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Block(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic.message() == "expected an outdent to close an indented block"
        }));
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
    fn recovers_from_an_unclosed_application_in_a_catch_case_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try x catch case E => f(",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Keyword(HardKeyword::Catch), 6, 11),
                token(TokenKind::Keyword(HardKeyword::Case), 12, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let id = parser.expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::ParsedTry(_))
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
    fn recognizes_multiple_unindented_catch_cases_in_a_braced_scope() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ try x catch\n  case A => a\n  case B => b\n}",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::Try), 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::Catch), 8, 13),
                token(TokenKind::Newline, 13, 14),
                token(TokenKind::Keyword(HardKeyword::Case), 16, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Operator, 23, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Newline, 27, 28),
                token(TokenKind::Keyword(HardKeyword::Case), 30, 34),
                token(TokenKind::Identifier, 35, 36),
                token(TokenKind::Operator, 37, 39),
                token(TokenKind::Identifier, 40, 41),
                token(TokenKind::Newline, 41, 42),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 42, 43),
                token(TokenKind::Eof, 43, 43),
            ],
            &mut names,
        );

        parser.advance();
        parser.advance();
        parser.advance();
        assert_eq!(
            parser.current().kind,
            TokenKind::Keyword(HardKeyword::Catch)
        );
        assert!(parser.catch_has_multiple_unindented_cases());
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
    fn parses_try_with_a_finally_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() finally cleanup()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Finally), 12, 19),
                token(TokenKind::Identifier, 20, 27),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 27, 28),
                token(TokenKind::Punctuation(Punctuation::RightParen), 28, 29),
                token(TokenKind::Eof, 29, 29),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: None,
            finalizer: Some(finalizer),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with a finalizer");
        };

        assert!(matches!(
            parser.ast().get(finalizer).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_indented_finally_statement_block() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() finally\n  val x = 1\n  cleanup(x)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Finally), 12, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Indent, 22, 22),
                token(TokenKind::Keyword(HardKeyword::Val), 22, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Operator, 28, 29),
                token(TokenKind::IntegerLiteral, 30, 31),
                token(TokenKind::Newline, 31, 32),
                token(TokenKind::Identifier, 34, 41),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 41, 42),
                token(TokenKind::Identifier, 42, 43),
                token(TokenKind::Punctuation(Punctuation::RightParen), 43, 44),
                token(TokenKind::Outdent, 44, 44),
                token(TokenKind::Eof, 44, 44),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            finalizer: Some(finalizer),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected a parsed try with an indented finalizer");
        };
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(finalizer).kind else {
            panic!("expected the indented finally statements to form a block");
        };

        assert_eq!(stats.len(), 1);
        assert!(matches!(
            parser.ast().get(stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Apply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser.diagnostics().is_empty(),
            "{:?}",
            parser.diagnostics()
        );
    }

    #[test]
    fn parses_try_with_catch_and_an_indented_finally_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch recover() finally\n  cleanup()",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Identifier, 18, 25),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 25, 26),
                token(TokenKind::Punctuation(Punctuation::RightParen), 26, 27),
                token(TokenKind::Keyword(HardKeyword::Finally), 28, 35),
                token(TokenKind::Indent, 38, 38),
                token(TokenKind::Identifier, 38, 45),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 45, 46),
                token(TokenKind::Punctuation(Punctuation::RightParen), 46, 47),
                token(TokenKind::Outdent, 47, 47),
                token(TokenKind::Eof, 47, 47),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: Some(handler),
            finalizer: Some(finalizer),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected parsed try tree with catch and finally");
        };

        assert!(matches!(parser.ast().get(handler).kind, TreeKind::Apply(_)));
        assert!(matches!(
            parser.ast().get(finalizer).kind,
            TreeKind::Apply(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_catch_without_a_handler_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: Some(handler),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected a recovered catch handler");
        };

        assert!(matches!(
            parser.ast().get(handler).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_an_empty_braced_catch_handler_as_a_block() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() catch {}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Catch), 12, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            handler: Some(handler),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected a recovered empty catch handler");
        };

        let TreeKind::Block(Block { ref stats, .. }) = parser.ast().get(handler).kind else {
            panic!("expected block handler");
        };
        assert!(stats.is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_finally_without_a_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "try risky() finally",
            vec![
                token(TokenKind::Keyword(HardKeyword::Try), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Finally), 12, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let id = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::ParsedTry(ParsedTry {
            finalizer: Some(finalizer),
            ..
        })) = parser.ast().get(id).kind
        else {
            panic!("expected a recovered finalizer");
        };

        assert!(matches!(
            parser.ast().get(finalizer).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
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
