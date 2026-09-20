use dotty_core::ast::{PolyFunction, UntypedNode};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{ParamOwner, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn starts_poly_function(&mut self) -> bool {
        self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket)
    }

    pub(super) fn parse_poly_function(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let type_params = self.parse_type_param_clause(ParamOwner::Type);
        if !self.current_is_arrow() {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` after polymorphic function type parameters",
            );
            return self.error_expr(self.current_span());
        }

        if self.arrow_starts_indented_body() {
            self.observe_arrow_indented();
        }
        self.advance();
        let body = self.parse_poly_function_body();

        if !matches!(
            self.ast.get(body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(_))
        ) {
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
                "polymorphic function literals require a value-parameter function body",
            );
            let position = self
                .ast
                .get(body)
                .position
                .unwrap_or_else(|| self.current_span());
            return self.error_expr(position);
        }

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::PolyFunction(PolyFunction {
                type_params,
                body,
            })),
        )
    }

    fn parse_poly_function_body(&mut self) -> TreeId<Untyped> {
        self.consume_lambda_newlines();
        if self.current().kind == TokenKind::Indent {
            self.parse_indented_block()
        } else {
            self.expr()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Function, PolyFunction, TypeDef, UntypedNode};
    use dotty_core::{NameInterner, TextRange, Token, TokenValue};

    #[test]
    fn parses_a_polymorphic_function_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => (x: A) => x",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(4, 6).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::ColonOp, 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(14, 16).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(PolyFunction {
            ref type_params,
            body,
        })) = parser.ast().get(tree).kind
        else {
            panic!("expected a polymorphic function");
        };
        assert_eq!(type_params.len(), 1);
        assert!(matches!(
            parser.ast().get(type_params[0]).kind,
            TreeKind::TypeDef(TypeDef { .. })
        ));
        assert!(matches!(
            parser.ast().get(body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Function(Function { .. }))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_multiple_polymorphic_type_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A, B] => (x: A) => x",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(7, 9).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::ColonOp, 12, 13),
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
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(PolyFunction {
            ref type_params,
            ..
        })) = parser.ast().get(tree).kind
        else {
            panic!("expected a polymorphic function");
        };
        assert_eq!(type_params.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_polymorphic_function_without_a_value_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => value",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(4, 6).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message()
                .contains("value-parameter function body")
        }));
    }
}
