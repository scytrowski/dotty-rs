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
