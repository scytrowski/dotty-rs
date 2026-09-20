use dotty_core::ast::{Assign, UntypedNode};
use dotty_core::{Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

mod control_flow;
mod match_expr;
mod operators;
mod simple;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a complete expression at the future `Expr` grammar boundary.
    ///
    /// Lambdas, polyfunctions, and placeholder expressions will extend this
    /// entry point in later milestones. For now `Expr` is exactly `Expr1`.
    pub(crate) fn expr(&mut self) -> TreeId<Untyped> {
        self.expr1()
    }

    fn expr1(&mut self) -> TreeId<Untyped> {
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::If) {
            let mark = self.mark();
            return self.parse_if_expr(mark);
        }
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::While) {
            let mark = self.mark();
            return self.parse_while_expr(mark);
        }
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Throw) {
            let mark = self.mark();
            return self.parse_throw_expr(mark);
        }
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Return) {
            let mark = self.mark();
            return self.parse_return_expr(mark);
        }

        let tree = self.postfix_expr();
        self.expr1_rest(tree)
    }

    fn expr1_rest(&mut self, lhs: TreeId<Untyped>) -> TreeId<Untyped> {
        if !self.current_is_bare_assignment() {
            return lhs;
        }

        self.advance();
        let rhs = self.expr();
        if !is_assignable_lhs(&self.ast.get(lhs).kind) {
            self.report(
                crate::ParseDiagnosticKind::UnexpectedToken,
                "left-hand side is not assignable",
            );
            return lhs;
        }

        self.alloc_assign(lhs, rhs)
    }

    fn current_is_bare_assignment(&mut self) -> bool {
        self.current().kind == TokenKind::Operator && self.current_text().ok() == Some("=")
    }

    fn alloc_assign(&mut self, lhs: TreeId<Untyped>, rhs: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(lhs)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let end = self
            .ast
            .get(rhs)
            .position
            .map(|position| position.span().range().end())
            .unwrap_or(self.last_real_token_end);
        let range = TextRange::new(start, end).expect("assignment child spans are ordered");
        self.alloc(
            TreeKind::Assign(Assign { lhs, rhs }),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }
}

fn is_assignable_lhs(kind: &TreeKind<Untyped>) -> bool {
    matches!(
        kind,
        TreeKind::Ident(_)
            | TreeKind::Select(_)
            | TreeKind::Apply(_)
            | TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_))
            | TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_))
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingOperator {
    name: dotty_core::Name,
    offset: u32,
}

const fn is_block_separator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Punctuation(Punctuation::Semicolon)
            | TokenKind::Indent
            | TokenKind::Outdent
    )
}

const fn is_else_separator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline | TokenKind::Newlines | TokenKind::Punctuation(Punctuation::Semicolon)
    )
}

const fn is_numeric_literal(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral
    )
}

const fn can_start_prefix_expr(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::Operator
            | TokenKind::Keyword(
                dotty_core::HardKeyword::True
                    | dotty_core::HardKeyword::False
                    | dotty_core::HardKeyword::Null
                    | dotty_core::HardKeyword::This
                    | dotty_core::HardKeyword::Super
                    | dotty_core::HardKeyword::New
            )
            | TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral
            | TokenKind::StringLiteral
            | TokenKind::Punctuation(Punctuation::LeftParen | Punctuation::LeftBrace)
    )
}

pub(super) const fn can_start_expr(kind: TokenKind) -> bool {
    can_start_prefix_expr(kind)
        || matches!(
            kind,
            TokenKind::Keyword(
                dotty_core::HardKeyword::If
                    | dotty_core::HardKeyword::While
                    | dotty_core::HardKeyword::Try
                    | dotty_core::HardKeyword::Throw
                    | dotty_core::HardKeyword::Return
            )
        )
}

#[cfg(test)]
mod tests;
