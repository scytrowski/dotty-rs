use dotty_core::ast::{Annotated, Assign, Function, TypedExpr, UntypedNode};
use dotty_core::{Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseKind, Parser};

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
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Do) {
            let mark = self.mark();
            return self.parse_do_while_expr(mark);
        }
        if self.current().kind == TokenKind::Identifier
            && self.current_text_is("inline")
            && (self.starts_inline_if() || self.starts_inline_match())
        {
            let mark = self.mark();
            return self.parse_inline_expr(mark);
        }
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

    fn starts_inline_if(&mut self) -> bool {
        self.cursor.lookahead(1).kind == TokenKind::Keyword(dotty_core::HardKeyword::If)
    }

    fn starts_inline_match(&mut self) -> bool {
        let mut nesting = [0u32; 3];
        let mut offset = 1;
        loop {
            let token = self.cursor.lookahead(offset);
            match token.kind {
                TokenKind::Keyword(dotty_core::HardKeyword::Match) if nesting == [0, 0, 0] => {
                    return true;
                }
                TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Indent
                | TokenKind::Outdent
                | TokenKind::Punctuation(Punctuation::Semicolon | Punctuation::Comma)
                    if nesting == [0, 0, 0] =>
                {
                    return false;
                }
                TokenKind::Punctuation(Punctuation::LeftParen) => nesting[0] += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    if nesting[0] == 0 {
                        return false;
                    }
                    nesting[0] -= 1;
                }
                TokenKind::Punctuation(Punctuation::LeftBracket) => nesting[1] += 1,
                TokenKind::Punctuation(Punctuation::RightBracket) => {
                    if nesting[1] == 0 {
                        return false;
                    }
                    nesting[1] -= 1;
                }
                TokenKind::Punctuation(Punctuation::LeftBrace) => nesting[2] += 1,
                TokenKind::Punctuation(Punctuation::RightBrace) => {
                    if nesting[2] == 0 {
                        return false;
                    }
                    nesting[2] -= 1;
                }
                _ => {}
            }
            offset += 1;
        }
    }

    fn parse_inline_expr(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::If) {
            let parsed = self.parse_if_expr(mark);
            if let TreeKind::If(if_expr) = &self.ast.get(parsed).kind {
                return self.alloc_from(
                    mark,
                    TreeKind::PhaseSpecific(UntypedNode::InlineIf(dotty_core::ast::InlineIf {
                        cond: if_expr.cond,
                        then_branch: if_expr.then_branch,
                        else_branch: if_expr.else_branch,
                    })),
                );
            }
            return parsed;
        }

        let parsed = self.expr();
        if let TreeKind::Match(match_expr) = &self.ast.get(parsed).kind {
            let selector_start = self
                .ast
                .get(match_expr.selector)
                .position
                .map(|position| position.span().range().start())
                .unwrap_or(mark.start());
            return self.alloc_from(
                crate::Mark {
                    start: selector_start,
                },
                TreeKind::PhaseSpecific(UntypedNode::InlineMatch(dotty_core::ast::InlineMatch {
                    selector: match_expr.selector,
                    cases: match_expr.cases.clone(),
                })),
            );
        }

        self.report(
            crate::ParseDiagnosticKind::ExpectedExpression,
            "expected `if` or a `match` expression after `inline`",
        );
        parsed
    }

    fn expr1_rest(&mut self, lhs: TreeId<Untyped>) -> TreeId<Untyped> {
        if self.current_is_bare_assignment() {
            let assignment_end = self.current().span.end();
            let feedback_indent = self.observe_definition_rhs_indentation();
            self.advance();
            let following_definition = self.definition_after_newlines();
            if following_definition.is_none() {
                self.consume_control_newlines();
            }
            let missing_rhs_start = following_definition.or_else(|| {
                self.starts_definition_statement()
                    .then(|| self.current().span.start())
            });
            let consumed_statement_separator = missing_rhs_start.is_some()
                && matches!(
                    self.context.location,
                    Location::Elsewhere | Location::InBlock
                )
                && self
                    .source
                    .as_str()
                    .get(assignment_end as usize..self.current().span.start() as usize)
                    .is_some_and(|gap| gap.chars().any(dotty_core::is_line_break_char));
            let rhs = if let Some(start) = missing_rhs_start {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedExpression,
                    "expected an expression after `=`",
                );
                self.error_expr(self.zero_width_span(start))
            } else if self.current().kind == TokenKind::Indent {
                if let Some(indent_offset) = feedback_indent {
                    self.parse_region_feedback_indented_block(indent_offset)
                } else {
                    self.parse_indented_block()
                }
            } else {
                self.expr()
            };
            if !is_assignable_lhs(&self.ast.get(lhs).kind) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "left-hand side is not assignable",
                );
                self.last_advance_consumed_statement_separator |= consumed_statement_separator;
                return lhs;
            }

            let assignment = self.alloc_assign(lhs, rhs);
            self.last_advance_consumed_statement_separator |= consumed_statement_separator;
            return assignment;
        }

        if self.current().kind == TokenKind::ColonFollow && self.colon_followed_by_indented_lambda()
        {
            return self.parse_colon_lambda_argument(lhs);
        }
        if self.current().kind == TokenKind::ColonFollow {
            self.observe_colon_eol(false);
        }
        if self.current_is_ascription_colon() {
            self.advance();
            return self.parse_ascription(lhs);
        }
        if self.current().kind == TokenKind::ColonEol {
            let application = self.parse_colon_argument(lhs);
            return self.expr1_rest(application);
        }
        lhs
    }

    /// Matches Dotty's `followingIsLambdaAfterColon` for the lambda forms
    /// supported by this parser. The lambda's own parser requests the layout
    /// region after `=>`; the colon must not open a colon-EOL argument region.
    fn colon_followed_by_indented_lambda(&mut self) -> bool {
        let mut offset = 1usize;
        match self.cursor.lookahead(offset).kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => offset += 1,
            TokenKind::Punctuation(Punctuation::LeftParen)
            | TokenKind::Punctuation(Punctuation::LeftBracket) => {
                let (open, close) = match self.cursor.lookahead(offset).kind {
                    TokenKind::Punctuation(Punctuation::LeftParen) => {
                        (Punctuation::LeftParen, Punctuation::RightParen)
                    }
                    _ => (Punctuation::LeftBracket, Punctuation::RightBracket),
                };
                let mut depth = 0usize;
                loop {
                    match self.cursor.lookahead(offset).kind {
                        TokenKind::Punctuation(kind) if kind == open => depth += 1,
                        TokenKind::Punctuation(kind) if kind == close => {
                            depth -= 1;
                            offset += 1;
                            if depth == 0 {
                                break;
                            }
                            continue;
                        }
                        TokenKind::Eof => return false,
                        _ => {}
                    }
                    offset += 1;
                }
            }
            _ => return false,
        }

        let arrow = self.cursor.lookahead(offset).clone();
        if arrow.kind != TokenKind::Operator
            || !matches!(self.source.slice(arrow.span).ok(), Some("=>" | "?=>"))
        {
            return false;
        }

        self.cursor
            .observe_at(offset, dotty_core::ScannerEvent::ArrowIndented);
        matches!(
            self.cursor.lookahead(offset + 1).kind,
            TokenKind::Indent | TokenKind::Eof
        )
    }

    fn parse_colon_lambda_argument(&mut self, function: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(function)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());
        let argument = self.parse_colon_lambda_body();
        let application = self.alloc_from(
            crate::Mark { start },
            TreeKind::Apply(dotty_core::ast::Apply {
                function,
                args: vec![argument],
                kind: dotty_core::ast::ApplyKind::Regular,
            }),
        );

        // As with a colon-EOL block argument, a lambda argument is an
        // application suffix. Dotty permits further selections/applications
        // after the indented lambda body has closed.
        self.simple_expr_rest(crate::Mark { start }, application, true)
    }

    pub(super) fn parse_colon_lambda_body(&mut self) -> TreeId<Untyped> {
        self.advance(); // ColonFollow
        self.with_location(crate::Location::InColonArg, |parser| parser.expr())
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

        if self.current().kind == TokenKind::Operator && self.current_text_is("@") {
            let mut tree = expr;
            while self.current().kind == TokenKind::Operator && self.current_text_is("@") {
                let annotation = self.parse_annotation();
                tree = self.alloc_from(
                    crate::Mark { start },
                    TreeKind::Annotated(Annotated {
                        expr: tree,
                        annotation,
                    }),
                );
            }
            return tree;
        }

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
            | TokenKind::InterpolationId
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
