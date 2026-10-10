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

        if self.get_function_body(body).is_none() {
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

    /// Matches Dotty's `getFunction`: syntactic parentheses and an empty
    /// expression block may wrap the value-parameter function body.
    fn get_function_body(&self, tree: TreeId<Untyped>) -> Option<TreeId<Untyped>> {
        match &self.ast.get(tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::Function(_)) => Some(tree),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                self.get_function_body(parens.inner)
            }
            TreeKind::Block(block) if block.stats.is_empty() => self.get_function_body(block.expr),
            _ => None,
        }
    }

    fn parse_poly_function_body(&mut self) -> TreeId<Untyped> {
        self.consume_lambda_newlines();
        if self.current().kind == TokenKind::Indent {
            // The arrow opened this scanner feedback region, so close it with
            // delimiter-aware feedback when the polyfunction is an argument.
            self.parse_feedback_indented_block()
        } else {
            self.expr()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{
        ContextBoundTypeTree, ContextBounds, Function, PolyFunction, TypeDef, UntypedNode,
    };
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
    fn preserves_context_bounds_in_a_polymorphic_function_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A: Show as show] => (x: A) => x",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Identifier, 9, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(18, 20).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Punctuation(Punctuation::LeftParen), 21, 22),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::ColonOp, 23, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Punctuation(Punctuation::RightParen), 26, 27),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(28, 30).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 31, 32),
                token(TokenKind::Eof, 32, 32),
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
        let TreeKind::TypeDef(parameter) = &parser.ast().get(type_params[0]).kind else {
            panic!("expected a type parameter definition");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(ContextBounds {
            context_bounds,
            ..
        })) = &parser.ast().get(parameter.rhs).kind
        else {
            panic!("expected preserved context bounds");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(ContextBoundTypeTree {
            name,
            ..
        })) = &parser.ast().get(context_bounds[0]).kind
        else {
            panic!("expected a context-bound type tree");
        };
        assert!(name.is_some());
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_variance_in_a_polymorphic_function_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[+A] => (x: A) => x",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(1, 2).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(5, 7).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Punctuation(Punctuation::LeftParen), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonOp, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(15, 17).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
                .legacy_message()
                .expect("legacy parser diagnostic")
                .contains("value-parameter function body")
        }));
    }

    #[test]
    fn accepts_a_parenthesized_value_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => ((x: A) => x)",
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
                token(TokenKind::Punctuation(Punctuation::LeftParen), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonOp, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(15, 17).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(PolyFunction { body, .. })) =
            parser.ast().get(tree).kind
        else {
            panic!("expected a polymorphic function");
        };
        assert!(matches!(
            parser.ast().get(body).kind,
            TreeKind::PhaseSpecific(UntypedNode::Parens(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_a_braced_empty_stats_value_function_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A] => { (x: A) => x }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(4, 6).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::ColonOp, 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(16, 18).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::PhaseSpecific(UntypedNode::PolyFunction(PolyFunction { body, .. })) =
            parser.ast().get(tree).kind
        else {
            panic!("expected a polymorphic function");
        };
        assert!(
            matches!(parser.ast().get(body).kind, TreeKind::Block(ref block) if block.stats.is_empty())
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_a_missing_type_parameter_closing_bracket_before_the_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A => body",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
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

        let _ = parser.expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            matches!(
                diagnostic.issue(),
                crate::ParseIssue::ExpectedToken {
                    expected: TokenKind::Punctuation(Punctuation::RightBracket),
                    found: TokenKind::Operator,
                }
            )
        }));
    }
}
