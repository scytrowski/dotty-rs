use dotty_core::ast::{
    Ident, Literal, NumberKind, NumberLiteral, Parens, This, Tuple, UntypedNode,
};
use dotty_core::{Constant, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

#[allow(dead_code)]
impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the deliberately small expression subset used by the smoke milestone.
    pub(crate) fn parse_smoke_expr(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();

        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let Ok(name) = self.intern_current_term_name() else {
                    return self.unexpected_expression();
                };
                self.advance();
                self.alloc_from(
                    mark,
                    TreeKind::Ident(Ident {
                        name: *name.as_name(),
                    }),
                )
            }
            TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral => self.parse_number(mark),
            TokenKind::StringLiteral => self.parse_string(mark),
            TokenKind::Keyword(dotty_core::HardKeyword::True) => {
                self.parse_literal(mark, Constant::Boolean(true))
            }
            TokenKind::Keyword(dotty_core::HardKeyword::False) => {
                self.parse_literal(mark, Constant::Boolean(false))
            }
            TokenKind::Keyword(dotty_core::HardKeyword::Null) => {
                self.parse_literal(mark, Constant::Null)
            }
            TokenKind::Keyword(dotty_core::HardKeyword::This) => {
                self.advance();
                self.alloc_from(mark, TreeKind::This(This { qual: None }))
            }
            TokenKind::Punctuation(Punctuation::LeftParen) => self.parse_parens_or_tuple(mark),
            _ => self.unexpected_expression(),
        }
    }

    fn parse_number(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let kind = match self.current().kind {
            TokenKind::IntegerLiteral | TokenKind::LongLiteral => NumberKind::Whole(10),
            TokenKind::DecimalLiteral => NumberKind::Decimal,
            TokenKind::ExponentLiteral | TokenKind::FloatLiteral | TokenKind::DoubleLiteral => {
                NumberKind::Floating
            }
            _ => return self.unexpected_expression(),
        };
        let Ok(text) = self.current_text() else {
            return self.unexpected_expression();
        };
        let text = self.names.intern(text);
        self.advance();
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Number(NumberLiteral { text, kind })),
        )
    }

    fn parse_string(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let Ok(text) = self.current_text() else {
            return self.unexpected_expression();
        };
        let Some(value) = text
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            return self.unexpected_expression();
        };
        let value = self.names.intern(value);
        self.advance();
        self.alloc_from(
            mark,
            TreeKind::Literal(Literal {
                value: Constant::String(value),
            }),
        )
    }

    fn parse_literal(&mut self, mark: crate::Mark, value: Constant) -> TreeId<Untyped> {
        self.advance();
        self.alloc_from(mark, TreeKind::Literal(Literal { value }))
    }

    fn parse_parens_or_tuple(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            return self.alloc_from(
                mark,
                TreeKind::Literal(Literal {
                    value: Constant::Unit,
                }),
            );
        }

        let first = self.parse_smoke_expr();
        if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner: first })),
            );
        }

        let mut elements = vec![first];
        while self.current().kind != TokenKind::Punctuation(Punctuation::RightParen)
            && self.current().kind != TokenKind::Eof
        {
            elements.push(self.parse_smoke_expr());
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }
        self.expect(TokenKind::Punctuation(Punctuation::RightParen));
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { elements })),
        )
    }

    fn unexpected_expression(&mut self) -> TreeId<Untyped> {
        let position = self.current_span();
        self.report(
            crate::ParseDiagnosticKind::ExpectedExpression,
            "expected a supported smoke expression",
        );
        if self.current().kind != TokenKind::Eof {
            self.advance();
        }
        self.error_expr(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{
        HardKeyword, NameInterner, SourceId, SourceText, TextRange, Token, TokenSource, TokenValue,
    };

    struct VecTokenSource {
        tokens: Vec<Token>,
        index: usize,
    }

    impl TokenSource for VecTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index]
        }

        fn advance(&mut self) {
            if self.index + 1 < self.tokens.len() {
                self.index += 1;
            }
        }

        fn lookahead(&mut self, n: usize) -> &Token {
            let index = self
                .index
                .saturating_add(n)
                .min(self.tokens.len().saturating_sub(1));
            &self.tokens[index]
        }

        fn observe(&mut self, _event: dotty_core::ScannerEvent) {}
    }

    fn token(kind: TokenKind, start: u32, end: u32) -> Token {
        Token {
            kind,
            span: TextRange::new(start, end).expect("valid test range"),
            value: TokenValue::None,
        }
    }

    fn parser_for<'src, 'names>(
        source: &'src str,
        tokens: Vec<Token>,
        names: &'names mut NameInterner,
    ) -> Parser<'src, 'names, VecTokenSource> {
        Parser::new(
            SourceText::new(source).expect("valid source"),
            SourceId::from_index(1),
            VecTokenSource { tokens, index: 0 },
            names,
        )
    }

    #[test]
    fn parses_an_identifier_with_its_source_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Ident(ident) = tree.kind else {
            panic!("expected identifier tree");
        };
        assert_eq!(names.resolve(ident.name.text()), "x");
        assert_eq!(
            tree.position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn parses_an_integer_as_a_raw_whole_number() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "42",
            vec![
                token(TokenKind::IntegerLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = tree.kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(10));
        assert_eq!(names.resolve(number.text), "42");
    }

    #[test]
    fn parses_a_string_literal_without_retaining_quote_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"hello\"",
            vec![
                token(TokenKind::StringLiteral, 0, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = tree.kind
        else {
            panic!("expected string literal tree");
        };
        assert_eq!(names.resolve(value), "hello");
    }

    #[test]
    fn parses_true_as_a_boolean_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "true",
            vec![
                token(TokenKind::Keyword(HardKeyword::True), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Boolean(true)
            })
        ));
    }

    #[test]
    fn parses_false_as_a_boolean_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "false",
            vec![
                token(TokenKind::Keyword(HardKeyword::False), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Boolean(false)
            })
        ));
    }

    #[test]
    fn parses_null_as_a_null_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "null",
            vec![
                token(TokenKind::Keyword(HardKeyword::Null), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Null
            })
        ));
    }

    #[test]
    fn parses_this_as_a_this_tree() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "this",
            vec![
                token(TokenKind::Keyword(HardKeyword::This), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::This(This { qual: None })
        ));
    }

    #[test]
    fn parses_parenthesized_expression_as_a_parens_node() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) =
            parser.ast().get(id).kind
        else {
            panic!("expected parens tree");
        };
        assert!(matches!(parser.ast().get(inner).kind, TreeKind::Ident(_)));
    }

    #[test]
    fn parses_empty_parentheses_as_unit() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "()",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightParen), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Unit
            })
        ));
    }

    #[test]
    fn parses_a_tuple_with_all_element_trees() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(a, b)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { ref elements })) =
            parser.ast().get(id).kind
        else {
            panic!("expected tuple tree");
        };
        assert_eq!(elements.len(), 2);
        assert!(matches!(
            parser.ast().get(elements[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(elements[1]).kind,
            TreeKind::Ident(_)
        ));
    }

    #[test]
    fn unsupported_expression_input_produces_an_error_tree_and_diagnostic() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "@",
            vec![token(TokenKind::Error, 0, 1), token(TokenKind::Eof, 1, 1)],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
    }
}
