use dotty_core::ast::{Assign, Function, TypedExpr, UntypedNode};
use dotty_core::{Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use crate::{ParseKind, Parser};

mod arguments;
mod control_flow;
mod for_expr;
mod interpolation;
mod lambda;
mod match_expr;
mod operators;
mod poly;
mod simple;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a complete expression at the `Expr` grammar boundary.
    pub(crate) fn expr(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let saved_placeholders = std::mem::take(&mut self.placeholder_params);
        let tree = if self.starts_poly_function() {
            self.parse_poly_function(mark)
        } else if self.starts_lambda() {
            self.parse_lambda(mark)
        } else {
            self.expr1()
        };
        let local_placeholders = std::mem::take(&mut self.placeholder_params);

        if self.is_placeholder_reference(tree, &local_placeholders) {
            let mut propagated = saved_placeholders;
            propagated.extend(local_placeholders);
            self.placeholder_params = propagated;
            return tree;
        }

        self.placeholder_params = saved_placeholders;
        if local_placeholders.is_empty() {
            return tree;
        }

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Function(Function {
                params: local_placeholders,
                body: tree,
            })),
        )
    }

    fn is_placeholder_reference(
        &self,
        tree: TreeId<Untyped>,
        placeholders: &[TreeId<Untyped>],
    ) -> bool {
        let Some(parameter) = placeholders.last() else {
            return false;
        };
        let TreeKind::ValDef(definition) = &self.ast.get(*parameter).kind else {
            return false;
        };
        match &self.ast.get(tree).kind {
            TreeKind::Ident(identifier) => identifier.name == *definition.name.as_name(),
            TreeKind::Typed(typed) => self.is_placeholder_reference(typed.expr, placeholders),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.is_placeholder_reference(parens.inner, placeholders)
            }
            _ => false,
        }
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
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Try) {
            let mark = self.mark();
            return self.parse_try_expr(mark);
        }
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Throw) {
            let mark = self.mark();
            return self.parse_throw_expr(mark);
        }
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Return) {
            let mark = self.mark();
            return self.parse_return_expr(mark);
        }
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::For) {
            let mark = self.mark();
            return self.parse_for_expr(mark);
        }

        let tree = self.postfix_expr();
        self.expr1_rest(tree)
    }

    fn expr1_rest(&mut self, lhs: TreeId<Untyped>) -> TreeId<Untyped> {
        if self.current_is_bare_assignment() {
            self.advance();
            let rhs = self.expr();
            if !is_assignable_lhs(&self.ast.get(lhs).kind) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "left-hand side is not assignable",
                );
                return lhs;
            }

            return self.alloc_assign(lhs, rhs);
        }

        if self.current().kind == TokenKind::ColonFollow {
            self.observe_colon_eol(false);
        }
        if self.current_is_ascription_colon() {
            self.advance();
            return self.parse_ascription(lhs);
        }
        if self.current().kind == TokenKind::ColonEol {
            return self.parse_colon_argument(lhs);
        }
        lhs
    }

    pub(crate) fn current_is_bare_assignment(&mut self) -> bool {
        self.current().kind == TokenKind::Operator && self.current_text().ok() == Some("=")
    }

    fn current_is_ascription_colon(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::ColonFollow | TokenKind::ColonOp
        )
    }

    fn parse_ascription(&mut self, expr: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(expr)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let tpt = self.with_parse_kind(ParseKind::Type, |parser| parser.type_expr());
        self.update_active_placeholder_type(expr, tpt);
        self.alloc_from(
            crate::Mark { start },
            TreeKind::Typed(TypedExpr { expr, tpt }),
        )
    }

    fn update_active_placeholder_type(&mut self, expr: TreeId<Untyped>, tpt: TreeId<Untyped>) {
        let Some(parameter) = self.placeholder_params.last().copied() else {
            return;
        };
        if !self.is_placeholder_reference(expr, std::slice::from_ref(&parameter)) {
            return;
        }
        let TreeKind::ValDef(definition) = &mut self.ast.get_mut(parameter).kind else {
            return;
        };
        definition.tpt = tpt;
        let Some(parameter_position) = self.ast.get(parameter).position else {
            return;
        };
        let start = parameter_position.span().range().start();
        let end = self
            .ast
            .get(tpt)
            .position
            .map(|position| position.span().range().end())
            .unwrap_or(start);
        let range = TextRange::new(start, end).expect("placeholder type span is ordered");
        self.ast.get_mut(parameter).position =
            Some(SourceSpan::new(self.source_id, Span::without_point(range)));
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

pub(crate) const fn can_start_prefix_expr(kind: TokenKind) -> bool {
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

pub(crate) const fn can_start_expr(kind: TokenKind) -> bool {
    can_start_prefix_expr(kind)
        || matches!(
            kind,
            TokenKind::Keyword(
                dotty_core::HardKeyword::If
                    | dotty_core::HardKeyword::While
                    | dotty_core::HardKeyword::Try
                    | dotty_core::HardKeyword::Throw
                    | dotty_core::HardKeyword::Return
                    | dotty_core::HardKeyword::For
            ) | TokenKind::Punctuation(Punctuation::LeftBracket)
        )
}

#[cfg(test)]
mod tests;
