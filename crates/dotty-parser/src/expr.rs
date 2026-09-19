use dotty_core::ast::{Apply, ApplyKind, Ident, Parens, Select, This, Tuple, UntypedNode};
use dotty_core::{Constant, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the deliberately small expression subset used by the smoke milestone.
    pub(crate) fn parse_smoke_expr(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();

        let tree = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return self.unexpected_expression();
                };
                self.advance();
                self.alloc_from(
                    mark,
                    TreeKind::Ident(Ident {
                        name: *name.as_name(),
                        backquoted,
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
        };

        self.parse_select_suffix(mark, tree)
    }

    fn parse_select_suffix(
        &mut self,
        mark: crate::Mark,
        mut qualifier: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        loop {
            if self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                let name = match self.current().kind {
                    TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                        match self.intern_current_term_name() {
                            Ok(name) => name,
                            Err(_) => return self.unexpected_expression(),
                        }
                    }
                    _ => {
                        self.report(
                            crate::ParseDiagnosticKind::ExpectedToken,
                            "expected an identifier after `.`",
                        );
                        return qualifier;
                    }
                };
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                self.advance();
                qualifier = self.alloc_from(
                    mark,
                    TreeKind::Select(Select {
                        qualifier,
                        name: *name.as_name(),
                        backquoted,
                    }),
                );
            } else if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftParen))
            {
                qualifier = self.parse_application(mark, qualifier);
            } else {
                break;
            }
        }
        qualifier
    }

    fn parse_application(
        &mut self,
        mark: crate::Mark,
        function: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        self.advance();
        let mut args = Vec::new();
        if !self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            loop {
                args.push(self.parse_smoke_expr());
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedExpression,
                        "expected an argument after `,`",
                    );
                    self.advance();
                    break;
                }
            }
        }
        self.alloc_from(
            mark,
            TreeKind::Apply(Apply {
                function,
                args,
                kind: ApplyKind::Regular,
            }),
        )
    }

    fn parse_parens_or_tuple(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            return self.alloc_from(
                mark,
                TreeKind::Literal(dotty_core::ast::Literal {
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

    pub(crate) fn unexpected_expression(&mut self) -> TreeId<Untyped> {
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
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Literal, Parens, This, Tuple, UntypedNode};
    use dotty_core::{HardKeyword, NameInterner, TextRange};

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
        assert!(!ident.backquoted);
    }

    #[test]
    fn parses_a_backquoted_identifier_without_its_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "`x`",
            vec![
                token(TokenKind::BackquotedIdentifier, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Ident(ident) = parser.ast().get(id).kind else {
            panic!("expected identifier tree");
        };

        assert!(ident.backquoted);
        assert_eq!(names.resolve(ident.name.text()), "x");
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
    fn parses_a_simple_selection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo.bar",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let (name, qualifier) = match parser.ast().get(id).kind {
            TreeKind::Select(selection) => (selection.name, selection.qualifier),
            _ => panic!("expected selection tree"),
        };
        let qualifier_is_ident = matches!(parser.ast().get(qualifier).kind, TreeKind::Ident(_));
        assert!(parser.diagnostics().is_empty());
        drop(parser);

        assert_eq!(names.resolve(name.text()), "bar");
        assert!(qualifier_is_ident);
    }

    #[test]
    fn parses_a_backquoted_selection_without_its_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo.`bar`",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::BackquotedIdentifier, 4, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected selection tree");
        };

        assert!(selection.backquoted);
        assert_eq!(names.resolve(selection.name.text()), "bar");
    }

    #[test]
    fn parses_a_simple_application_with_an_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo(42)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::IntegerLiteral, 4, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert_eq!(application.args.len(), 1);
        assert!(matches!(
            parser.ast().get(application.function).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_empty_application_with_no_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo()",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightParen), 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert!(application.args.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_application_with_multiple_arguments_in_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo(1, 2)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::IntegerLiteral, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::IntegerLiteral, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert_eq!(application.args.len(), 2);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(matches!(
            parser.ast().get(application.args[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
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
