use dotty_core::ast::{Function, Modifiers, UntypedNode, ValDef};
use dotty_core::{
    Punctuation, SourceSpan, Span, TermName, TextRange, TokenKind, TreeId, TreeKind, Untyped,
};

use super::can_start_expr;
use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn starts_lambda(&mut self) -> bool {
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                self.cursor.lookahead(1).kind == TokenKind::Operator
                    && (self.lookahead_text_is(1, "=>") || self.lookahead_text_is(1, "?=>"))
            }
            TokenKind::Punctuation(Punctuation::LeftParen) => {
                self.lambda_arrow_after_parenthesized_params()
            }
            _ => false,
        }
    }

    fn lambda_arrow_after_parenthesized_params(&mut self) -> bool {
        let mut depth = 0usize;
        let mut offset = 0usize;
        loop {
            let token = self.cursor.lookahead(offset);
            match token.kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => depth += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let next_kind = self.cursor.lookahead(offset + 1).kind;
                        return next_kind == TokenKind::Operator
                            && (self.lookahead_text_is(offset + 1, "=>")
                                || self.lookahead_text_is(offset + 1, "?=>"));
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset = offset.saturating_add(1);
        }
    }

    fn lookahead_text_is(&mut self, offset: usize, expected: &str) -> bool {
        self.source.slice(self.cursor.lookahead(offset).span).ok() == Some(expected)
    }

    pub(super) fn parse_lambda(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let params = self.parse_fun_params();
        let context_arrow = self.current_is_context_arrow();
        if context_arrow && params.is_empty() {
            self.report(
                ParseDiagnosticKind::ExpectedExpression,
                "context function literals require at least one formal parameter",
            );
        }
        if context_arrow {
            self.add_given_to_params(&params);
        }

        if self.current_is_arrow() || context_arrow {
            if self.arrow_starts_indented_body() {
                self.observe_arrow_indented();
            }
            self.advance();
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` after lambda parameters",
            );
        }

        let body = self.parse_lambda_body();
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Function(Function { params, body })),
        )
    }

    fn parse_fun_params(&mut self) -> Vec<TreeId<Untyped>> {
        if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
            self.parse_parenthesized_fun_params()
        } else {
            vec![self.parse_binding()]
        }
    }

    fn parse_parenthesized_fun_params(&mut self) -> Vec<TreeId<Untyped>> {
        self.advance();
        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            return Vec::new();
        }

        let mut params = Vec::new();
        loop {
            if self.current().kind == TokenKind::Punctuation(Punctuation::Comma) {
                self.report(
                    ParseDiagnosticKind::ExpectedExpression,
                    "expected a lambda parameter",
                );
                self.advance();
                continue;
            }
            if self.current().kind == TokenKind::Punctuation(Punctuation::RightParen) {
                self.advance();
                break;
            }
            if self.current_is_arrow() || self.current_is_context_arrow() {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `)` after lambda parameters",
                );
                break;
            }

            params.push(self.parse_binding());
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                continue;
            }
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                break;
            }

            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `,` or `)` after lambda parameter",
            );
            self.recover_lambda_params();
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                continue;
            }
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen))
                || self.current_is_arrow()
                || self.current_is_context_arrow()
            {
                break;
            }
        }
        params
    }

    fn parse_binding(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let name = if self.current().kind != TokenKind::BackquotedIdentifier
            && self.current().kind == TokenKind::Identifier
            && self.current_text_is("_")
        {
            self.advance();
            self.fresh_wildcard_param_name()
        } else if matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            match self.intern_current_term_name() {
                Ok(name) => {
                    self.advance();
                    name
                }
                Err(_) => self.missing_binding_name(),
            }
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedExpression,
                "expected an identifier or `_` in lambda parameters",
            );
            self.missing_binding_name()
        };

        let type_tree = if self.accept_lambda_colon() {
            self.with_parse_kind(ParseKind::Type, |parser| parser.simple_type())
        } else {
            self.synthetic_type_tree()
        };

        self.alloc_from(
            mark,
            TreeKind::ValDef(ValDef {
                name,
                tpt: type_tree,
                rhs: None,
                metadata: Modifiers::default(),
            }),
        )
    }

    fn missing_binding_name(&mut self) -> TermName {
        if self.current().kind != TokenKind::Eof
            && !matches!(
                self.current().kind,
                TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightParen)
            )
            && !self.current_is_arrow()
            && !self.current_is_context_arrow()
        {
            self.advance();
        }
        self.fresh_wildcard_param_name()
    }

    fn accept_lambda_colon(&mut self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::ColonOp | TokenKind::ColonFollow | TokenKind::ColonEol
        ) && {
            self.advance();
            true
        }
    }

    fn recover_lambda_params(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightParen) | TokenKind::Eof
        ) && !self.current_is_arrow()
            && !self.current_is_context_arrow()
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn add_given_to_params(&mut self, params: &[TreeId<Untyped>]) {
        for parameter in params {
            if let TreeKind::ValDef(definition) = &mut self.ast.get_mut(*parameter).kind {
                definition
                    .metadata
                    .modifiers
                    .push(dotty_core::ast::Modifier::Given);
            }
        }
    }

    fn parse_lambda_body(&mut self) -> TreeId<Untyped> {
        self.consume_lambda_newlines();
        if self.current().kind == TokenKind::Indent {
            return self.parse_indented_block();
        }

        if self.context.location == Location::InBlock
            && let Some(end) = self.context.block_end
        {
            return self.parse_lambda_block_body(end);
        }

        if can_start_expr(self.current().kind) {
            self.expr()
        } else {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedExpression,
                "expected an expression after lambda arrow",
            );
            self.error_expr(position)
        }
    }

    fn parse_lambda_block_body(&mut self, end: TokenKind) -> TreeId<Untyped> {
        let mark = self.mark();
        let (stats, expr) = self.parse_expression_block_body(end);
        if stats.is_empty() {
            expr
        } else {
            self.alloc_from(
                mark,
                TreeKind::Block(dotty_core::ast::Block { stats, expr }),
            )
        }
    }

    fn consume_lambda_newlines(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) && self.cursor.lookahead(1).kind == TokenKind::Indent
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn arrow_starts_indented_body(&mut self) -> bool {
        matches!(
            self.cursor.lookahead(1).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) && can_start_expr(self.cursor.lookahead(2).kind)
    }

    fn fresh_wildcard_param_name(&mut self) -> TermName {
        let index = self.next_wildcard_param;
        self.next_wildcard_param = self.next_wildcard_param.saturating_add(1);
        let name = format!("$lambda_wildcard_{index}");
        TermName::new(self.names.intern(&name))
    }

    fn synthetic_type_tree(&mut self) -> TreeId<Untyped> {
        self.synthetic_type_tree_at(self.current().span.start())
    }

    fn synthetic_type_tree_at(&mut self, start: u32) -> TreeId<Untyped> {
        let range = TextRange::new(start, start).expect("zero-width synthetic type range");
        self.alloc(
            TreeKind::TypeTree(dotty_core::ast::TypeTree),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Function, ValDef};
    use dotty_core::{NameInterner, TextRange, Token, TokenValue};

    #[test]
    fn parses_a_single_parameter_function_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x => x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(2, 4).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(Function { params, body })) =
            parser.ast().get(tree).kind.clone()
        else {
            panic!("expected a function literal");
        };
        assert_eq!(params.len(), 1);
        assert!(matches!(
            parser.ast().get(params[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(parser.ast().get(body).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.ast().get(tree).position.unwrap().span().range(),
            TextRange::new(0, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_empty_parenthesized_function_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "() => unit",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightParen), 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected a function literal");
        };
        assert!(function.params.is_empty());
        assert!(matches!(
            parser.ast().get(function.body).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_typed_function_parameters_with_type_namespace_names() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, y: pkg.B) => y",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonOp, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonOp, 8, 9),
                token(TokenKind::Identifier, 10, 13),
                token(TokenKind::Punctuation(Punctuation::Dot), 13, 14),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(17, 19).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected a function literal");
        };
        assert_eq!(function.params.len(), 2);
        for parameter in &function.params {
            let TreeKind::ValDef(ValDef { rhs, .. }) = &parser.ast().get(*parameter).kind else {
                panic!("expected a ValDef parameter");
            };
            assert!(rhs.is_none());
        }
        let TreeKind::Select(selection) = parser
            .ast()
            .get(match parser.ast().get(function.params[1]).kind {
                TreeKind::ValDef(ref definition) => definition.tpt,
                _ => unreachable!(),
            })
            .kind
        else {
            panic!("expected a qualified type tree");
        };
        assert!(selection.name.is_type());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_context_function_literal_with_given_parameter_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "ctx ?=> use(ctx)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(4, 7).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::Function(ref function)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected a context function literal");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(function.params[0]).kind else {
            panic!("expected a ValDef parameter");
        };
        assert_eq!(
            parameter.metadata.modifiers,
            vec![dotty_core::ast::Modifier::Given]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_an_empty_context_function_parameter_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "() ?=> body",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightParen), 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 6).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 7, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ));
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message()
                .contains("at least one formal parameter")
        }));
    }
}
