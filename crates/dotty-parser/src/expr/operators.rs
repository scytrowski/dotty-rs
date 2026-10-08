use dotty_core::ast::{Ident, InfixOp, PrefixOp, TypedExpr, UntypedNode};
use dotty_core::{SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, TypeName, Untyped};

use super::simple::is_term_operator_identifier;
use super::{PendingOperator, can_start_prefix_expr, is_numeric_literal};
use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses an expression at the current operator-expression boundary.
    pub(crate) fn postfix_expr(&mut self) -> TreeId<Untyped> {
        let first = self.prefix_expr();
        let tree = self.infix_expr(first);
        if self.is_argument_vararg_splice() {
            self.parse_argument_vararg_splice(tree)
        } else {
            tree
        }
    }

    pub(super) fn infix_expr(&mut self, mut top: TreeId<Untyped>) -> TreeId<Untyped> {
        let mut operators = Vec::new();

        loop {
            self.consume_guard_infix_newlines();
            // Newlines are normally suppressed inside parentheses by the
            // scanner. Dotty's `InFor` separator region restores them between
            // enumerators; recognize that narrow boundary before treating an
            // identifier such as `pipe` or `fiber` as an infix method name.
            if self.for_enumerator_rhs && self.current_starts_multiline_for_enumerator() {
                break;
            }
            // The scanner suppresses a physical newline when an outdent
            // closes a nested layout region. An alphabetic identifier at
            // that boundary starts the enclosing statement, not an infix
            // continuation. Symbolic operators still follow the Scala
            // continuation rule below.
            if self.last_advance_was_outdent
                && matches!(
                    self.current().kind,
                    TokenKind::Identifier | TokenKind::BackquotedIdentifier
                )
            {
                break;
            }
            if self.is_argument_vararg_splice() {
                break;
            }
            if self.current().kind == TokenKind::ColonFollow
                && self.colon_followed_by_indented_lambda()
            {
                top = self.parse_colon_lambda_argument(top);
                continue;
            }
            if self.is_nonfinal_argument_spread() {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "spread operator `*` not allowed here; must come last in a parameter list",
                );
                self.advance();
                break;
            }
            let Some(operator) = self.current_infix_operator() else {
                break;
            };
            let checkpoint = self.cursor.checkpoint();
            if self.features().postfix_ops && !self.operator_has_following_operand() {
                self.advance();
                if !self.cursor.progressed_since(checkpoint) {
                    self.report(
                        crate::ParseDiagnosticKind::UnexpectedToken,
                        "parser made no progress while parsing a postfix operator",
                    );
                    break;
                }
                top = self.reduce_operator_stack(&mut operators, top, 0, true, None);
                return self.alloc_postfix(top, operator.name);
            }

            let operator_precedence = {
                let spelling = self.names.resolve(operator.name.text());
                crate::infix::precedence(spelling)
            };
            let operator_left_associative = {
                let spelling = self.names.resolve(operator.name.text());
                !crate::infix::is_right_associative(spelling)
            };

            top = self.reduce_operator_stack(
                &mut operators,
                top,
                operator_precedence,
                operator_left_associative,
                Some(operator.name),
            );
            self.advance();
            operators.push(crate::OpInfo {
                operand: top,
                operator: operator.name,
                offset: operator.offset,
            });
            if self.current().kind == TokenKind::ColonFollow
                && self.colon_followed_by_indented_lambda()
            {
                top = self.parse_colon_lambda_body();
            } else {
                if self.current().kind == TokenKind::ColonFollow {
                    self.observe_colon_eol(false);
                }
                top = if self.current().kind == TokenKind::ColonEol {
                    self.parse_colon_argument_body()
                } else {
                    self.consume_infix_newlines();
                    self.prefix_expr()
                };
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing an infix expression",
                );
                break;
            }
        }

        let mut tree = self.reduce_operator_stack(&mut operators, top, 0, true, None);
        loop {
            if self.current().kind != TokenKind::Keyword(dotty_core::HardKeyword::Match) {
                let mut lookahead = 1;
                while matches!(
                    self.cursor.lookahead(lookahead).kind,
                    TokenKind::Newline | TokenKind::Newlines
                ) {
                    lookahead += 1;
                }
                if self.cursor.lookahead(lookahead).kind
                    != TokenKind::Keyword(dotty_core::HardKeyword::Match)
                {
                    break;
                }
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
            if self.current().kind != TokenKind::Keyword(dotty_core::HardKeyword::Match) {
                break;
            }
            tree = self.parse_match_clause(tree);
        }
        tree
    }

    fn is_argument_vararg_splice(&mut self) -> bool {
        if self.context.location != crate::Location::InArgs || !self.current_text_is("*") {
            return false;
        }

        match self.cursor.lookahead(1).kind {
            TokenKind::Punctuation(dotty_core::Punctuation::RightParen) => true,
            TokenKind::Punctuation(dotty_core::Punctuation::Comma) => matches!(
                self.cursor.lookahead(2).kind,
                TokenKind::Punctuation(dotty_core::Punctuation::RightParen) | TokenKind::Eof
            ),
            _ => false,
        }
    }

    fn is_nonfinal_argument_spread(&mut self) -> bool {
        self.context.location == crate::Location::InArgs
            && self.current_text_is("*")
            && self.cursor.lookahead(1).kind
                == TokenKind::Punctuation(dotty_core::Punctuation::Comma)
            && !matches!(
                self.cursor.lookahead(2).kind,
                TokenKind::Punctuation(dotty_core::Punctuation::RightParen) | TokenKind::Eof
            )
    }

    fn parse_argument_vararg_splice(&mut self, expr: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(expr)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let star_span = self.current_span();
        self.advance();

        let wildcard_star = TypeName::new(self.names.intern("_*"));
        let tpt = self.alloc(
            TreeKind::Ident(Ident {
                name: *wildcard_star.as_name(),
                backquoted: false,
            }),
            Some(star_span),
        );
        self.alloc_from(
            crate::Mark { start },
            TreeKind::Typed(TypedExpr { expr, tpt }),
        )
    }

    fn current_infix_operator(&mut self) -> Option<PendingOperator> {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::Operator
                | TokenKind::ColonOp
        ) {
            return None;
        }

        // A standalone colon after a literal is lexed as `ColonOp` because
        // the scanner cannot classify it from the preceding token alone.
        // It is an expression ascription, not an infix operator.  Leave it
        // for `expr1_rest` so `1: Int` and `a + b: Int` become `Typed` trees.
        if self.current().kind == TokenKind::ColonOp && self.current_text_is(":") {
            return None;
        }

        if self.current_is_structural_operator() {
            return None;
        }

        let name = *self.intern_current_term_name().ok()?.as_name();
        Some(PendingOperator {
            name,
            offset: self.current().span.start(),
        })
    }

    fn consume_infix_newlines(&mut self) {
        if !matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) || !can_start_prefix_expr(self.cursor.lookahead(1).kind)
        {
            return;
        }

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

    fn consume_guard_infix_newlines(&mut self) {
        if self.context.location != crate::Location::InGuard {
            return;
        }

        if !matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            return;
        }
        // The scanner preserves the match-case region while parsing a guard
        // and exposes a same-indent operator continuation as a newline.
        let mut operator_offset = 0;
        while matches!(
            self.cursor.lookahead(operator_offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            operator_offset = operator_offset.saturating_add(1);
        }
        let operator = self.cursor.lookahead(operator_offset);
        if !matches!(operator.kind, TokenKind::Operator | TokenKind::ColonOp) {
            return;
        }

        let spelling = self.source.slice(operator.span).unwrap_or_default();
        if matches!(spelling, "=" | "=>") {
            return;
        }

        let mut operand_offset = operator_offset.saturating_add(1);
        if matches!(
            self.cursor.lookahead(operand_offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            operand_offset = operand_offset.saturating_add(1);
        }
        if !can_start_prefix_expr(self.cursor.lookahead(operand_offset).kind) {
            return;
        }

        for _ in 0..operator_offset {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn operator_has_following_operand(&mut self) -> bool {
        match self.cursor.lookahead(1).kind {
            TokenKind::Newline | TokenKind::Newlines => {
                can_start_prefix_expr(self.cursor.lookahead(2).kind)
            }
            kind => can_start_prefix_expr(kind),
        }
    }

    fn reduce_operator_stack(
        &mut self,
        operators: &mut Vec<crate::OpInfo>,
        mut top: TreeId<Untyped>,
        precedence: u8,
        left_associative: bool,
        next_operator: Option<dotty_core::Name>,
    ) -> TreeId<Untyped> {
        if let (Some(stack_top), Some(next_operator)) = (operators.last(), next_operator) {
            let stack_spelling = self.names.resolve(stack_top.operator.text()).to_owned();
            let next_spelling = self.names.resolve(next_operator.text()).to_owned();
            if crate::infix::has_mixed_associativity(&stack_spelling, &next_spelling) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    format!(
                        "mixed left- and right-associative operators `{stack_spelling}` and `{next_spelling}`"
                    ),
                );
            }
        }

        while let Some(stack_top) = operators.last() {
            let stack_spelling = self.names.resolve(stack_top.operator.text());
            let stack_precedence = crate::infix::precedence(stack_spelling);
            if !(precedence < stack_precedence
                || left_associative && precedence == stack_precedence)
            {
                break;
            }

            let stack_top = operators.pop().expect("operator stack was non-empty");
            top = self.alloc_infix(stack_top.operand, stack_top.operator, top);
        }
        top
    }

    pub(crate) fn alloc_infix(
        &mut self,
        left: TreeId<Untyped>,
        operator: dotty_core::Name,
        right: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(left)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let end = self
            .ast
            .get(right)
            .position
            .map(|position| position.span().range().end())
            .unwrap_or(self.last_real_token_end);
        let range = TextRange::new(start, end).expect("infix child spans are ordered");
        self.alloc(
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(InfixOp {
                left,
                op: operator,
                right,
            })),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }

    fn alloc_postfix(
        &mut self,
        operand: TreeId<Untyped>,
        operator: dotty_core::Name,
    ) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(operand)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let end = self.last_real_token_end;
        let range = TextRange::new(start, end).expect("postfix span endpoints are ordered");
        self.alloc(
            TreeKind::PhaseSpecific(UntypedNode::PostfixOp(dotty_core::ast::PostfixOp {
                operand,
                op: operator,
            })),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }

    fn prefix_expr(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let Some(operator) = self.current_prefix_operator() else {
            return self.simple_expr();
        };
        let is_negated_number = self.current_text().ok() == Some("-")
            && is_numeric_literal(self.cursor.lookahead(1).kind);
        let operator_end = self.current().span.end();
        let operator_position = self.current_span();
        self.advance();

        if matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) || self.has_physical_line_break(operator_end, self.current().span.start())
        {
            self.report(
                crate::ParseDiagnosticKind::ExpectedExpression,
                "a prefix operator must be followed by its operand on the same line",
            );
            return self.error_expr(operator_position);
        }

        if is_negated_number {
            let number = self.parse_negative_number(mark);
            return self.simple_expr_rest(mark, number, true);
        }

        let operand = self.simple_expr();
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::PrefixOp(PrefixOp {
                op: *operator.as_name(),
                operand,
            })),
        )
    }

    fn current_prefix_operator(&mut self) -> Option<dotty_core::TermName> {
        if self.current().kind != TokenKind::Operator {
            return None;
        }

        if !matches!(self.current_text().ok()?, "-" | "+" | "~" | "!") {
            return None;
        }

        let operand = self.cursor.lookahead(1).clone();
        if !can_start_prefix_expr(operand.kind)
            || (operand.kind == TokenKind::Operator
                && !self
                    .token_text(&operand)
                    .ok()
                    .is_some_and(is_term_operator_identifier))
            || self.has_physical_line_break(self.current().span.end(), operand.span.start())
        {
            return None;
        }

        self.intern_current_term_name().ok()
    }

    pub(crate) fn has_physical_line_break(&self, start: u32, end: u32) -> bool {
        self.source
            .as_str()
            .get(start as usize..end as usize)
            .map(|text| text.chars().any(dotty_core::is_line_break_char))
            .unwrap_or(true)
    }
}
