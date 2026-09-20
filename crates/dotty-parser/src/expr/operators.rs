use dotty_core::ast::{InfixOp, PrefixOp, UntypedNode};
use dotty_core::{SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use super::{PendingOperator, can_start_prefix_expr, is_numeric_literal};
use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses an expression at the current operator-expression boundary.
    pub(crate) fn postfix_expr(&mut self) -> TreeId<Untyped> {
        let first = self.prefix_expr();
        self.infix_expr(first)
    }

    pub(super) fn infix_expr(&mut self, mut top: TreeId<Untyped>) -> TreeId<Untyped> {
        let mut operators = Vec::new();

        while let Some(operator) = self.current_infix_operator() {
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
            self.consume_infix_newlines();
            top = self.prefix_expr();

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing an infix expression",
                );
                break;
            }
        }

        let mut tree = self.reduce_operator_stack(&mut operators, top, 0, true, None);
        while self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Match) {
            tree = self.parse_match_clause(tree);
        }
        tree
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

        match self.current_text().ok()? {
            "-" | "+" | "~" | "!" => self.intern_current_term_name().ok(),
            _ => None,
        }
    }

    fn has_physical_line_break(&self, start: u32, end: u32) -> bool {
        self.source
            .as_str()
            .get(start as usize..end as usize)
            .map(|text| text.chars().any(dotty_core::is_line_break_char))
            .unwrap_or(true)
    }
}
