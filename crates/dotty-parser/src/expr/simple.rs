use dotty_core::ast::{
    Apply, ApplyKind, Block, Ident, New, Parens, Quote, Select, Super, This, Tuple, UntypedNode,
    ValDef,
};
use dotty_core::{
    Constant, Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped,
};

use crate::Parser;
use crate::statements::StatementSequenceBoundary;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses one simple expression and all of its currently supported suffixes.
    pub(crate) fn simple_expr(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let tree = self.simple_expr_atom(mark);
        let can_apply = !matches!(self.ast.get(tree).kind, TreeKind::Block(_));

        self.simple_expr_rest(mark, tree, can_apply)
    }

    fn simple_expr_atom(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        if matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) && self.cursor.lookahead(1).kind == TokenKind::Punctuation(Punctuation::Dot)
            && self.cursor.lookahead(2).kind == TokenKind::Keyword(dotty_core::HardKeyword::Super)
        {
            return self.parse_qualified_super(mark);
        }

        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                if self.current().kind == TokenKind::Identifier && self.current_text_is("_") {
                    return self.parse_placeholder(mark);
                }
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
            TokenKind::InterpolationId => self.parse_interpolated_string(mark),
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
            TokenKind::Keyword(dotty_core::HardKeyword::Super) => self.parse_super(mark, None),
            TokenKind::Keyword(dotty_core::HardKeyword::New) => self.parse_new(mark),
            TokenKind::Quote => self.parse_quote(mark),
            TokenKind::Punctuation(Punctuation::LeftParen) => self.parse_parens_or_tuple(mark),
            TokenKind::Punctuation(Punctuation::LeftBrace) => self.parse_block(mark),
            _ => self.unexpected_expression(),
        }
    }

    fn parse_quote(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        match self.current().kind {
            TokenKind::Punctuation(Punctuation::LeftBrace) => {
                let body_mark = self.mark();
                self.advance();
                let (stats, expr) = self
                    .parse_expression_block_body(TokenKind::Punctuation(Punctuation::RightBrace));
                if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedToken,
                        "expected `}` to close quoted expression",
                    );
                }
                let body = if stats.is_empty() {
                    expr
                } else {
                    self.alloc_from(body_mark, TreeKind::Block(Block { stats, expr }))
                };
                self.alloc_from(
                    mark,
                    TreeKind::Quote(Quote {
                        body,
                        tags: Vec::new(),
                    }),
                )
            }
            TokenKind::Punctuation(Punctuation::LeftBracket) => {
                self.advance();
                let body =
                    self.with_parse_kind(crate::ParseKind::Type, |parser| parser.type_expr());
                if !self.accept(TokenKind::Punctuation(Punctuation::RightBracket)) {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedToken,
                        "expected `]` to close quoted type",
                    );
                }
                self.alloc_from(
                    mark,
                    TreeKind::Quote(Quote {
                        body,
                        tags: Vec::new(),
                    }),
                )
            }
            _ => {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `{` or `[` after quote marker",
                );
                let body = self.error_expr(self.current_span());
                self.alloc_from(
                    mark,
                    TreeKind::Quote(Quote {
                        body,
                        tags: Vec::new(),
                    }),
                )
            }
        }
    }

    fn parse_placeholder(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let name = self.fresh_wildcard_param_name();
        let tpt = self.synthetic_type_tree_at(mark.start());
        let parameter = self.alloc(
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs: None,
                metadata: dotty_core::ast::Modifiers::default(),
            }),
            Some(SourceSpan::new(
                self.source_id,
                Span::without_point(TextRange::new(mark.start(), mark.start()).unwrap()),
            )),
        );
        self.placeholder_params.push(parameter);
        self.alloc_from(
            mark,
            TreeKind::Ident(Ident {
                name: *name.as_name(),
                backquoted: false,
            }),
        )
    }

    fn parse_block(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let (stats, expr) =
            self.parse_expression_block_body(TokenKind::Punctuation(Punctuation::RightBrace));
        if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected `}` to close block",
            );
        }
        self.alloc_from(mark, TreeKind::Block(Block { stats, expr }))
    }

    pub(crate) fn parse_expression_block_body(
        &mut self,
        end: TokenKind,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        self.with_enum_body(false, |parser| {
            parser.with_placeholder_scope(|parser| {
                parser.with_block_end(Some(end), |parser| {
                    parser.parse_statement_sequence(StatementSequenceBoundary::Block(end))
                })
            })
        })
    }

    pub(crate) fn synthetic_unit(&mut self) -> TreeId<Untyped> {
        self.synthetic_unit_at(self.current().span.start())
    }

    pub(super) fn synthetic_unit_at(&mut self, start: u32) -> TreeId<Untyped> {
        let range = TextRange::new(start, start).expect("zero-width synthetic unit range");
        self.alloc(
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Unit,
            }),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }

    pub(crate) fn parse_qualified_super(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let qualifier_mark = self.mark();
        let Ok(name) = self.intern_current_type_name() else {
            return self.unexpected_expression();
        };
        self.advance();
        self.expect(TokenKind::Punctuation(Punctuation::Dot));

        let qualifier = self.alloc_from(
            qualifier_mark,
            TreeKind::This(This {
                qual: Some(*name.as_name()),
            }),
        );
        if !self.accept(TokenKind::Keyword(dotty_core::HardKeyword::Super)) {
            return self.unexpected_expression();
        }
        self.parse_super_tail(mark, qualifier)
    }

    pub(crate) fn parse_super(
        &mut self,
        mark: crate::Mark,
        qualifier: Option<TreeId<Untyped>>,
    ) -> TreeId<Untyped> {
        let qualifier = qualifier.unwrap_or_else(|| {
            let position = self.current_span();
            self.advance();
            self.alloc(TreeKind::This(This { qual: None }), Some(position))
        });
        self.parse_super_tail(mark, qualifier)
    }

    pub(crate) fn parse_super_tail(
        &mut self,
        mark: crate::Mark,
        qualifier: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        let mix = if self.accept(TokenKind::Punctuation(Punctuation::LeftBracket)) {
            let mix = match self.current().kind {
                TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                    let Ok(name) = self.intern_current_type_name() else {
                        return self.unexpected_expression();
                    };
                    self.advance();
                    Some(*name.as_name())
                }
                _ => {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedType,
                        "expected a super type qualifier",
                    );
                    None
                }
            };
            self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
            mix
        } else {
            None
        };

        let super_tree = self.alloc_from(
            mark,
            TreeKind::Super(Super {
                qual: qualifier,
                mix,
            }),
        );

        if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected a selector after `super`",
            );
            return super_tree;
        }

        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let name = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_term_name() {
                    Ok(name) => name,
                    Err(_) => return super_tree,
                }
            }
            _ => {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected an identifier after `super.`",
                );
                return super_tree;
            }
        };
        self.advance();
        self.alloc_from(
            mark,
            TreeKind::Select(Select {
                qualifier: super_tree,
                name: *name.as_name(),
                backquoted,
            }),
        )
    }

    fn parse_new(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let type_mark = self.mark();
        let tpt = self.simple_type();
        let tpt = if self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::LeftBracket))
        {
            self.parse_type_application(type_mark, tpt)
        } else {
            tpt
        };
        let new_tree = self.alloc_from(type_mark, TreeKind::New(New { tpt }));
        if self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::LeftParen))
        {
            new_tree
        } else {
            let constructor = self.constructor_select(new_tree);
            self.alloc_from(
                mark,
                TreeKind::Apply(Apply {
                    function: constructor,
                    args: Vec::new(),
                    kind: ApplyKind::Regular,
                }),
            )
        }
    }

    pub(crate) fn constructor_select(&mut self, function: TreeId<Untyped>) -> TreeId<Untyped> {
        let position = self.ast.get(function).position;
        let name = dotty_core::TermName::new(self.names.intern("<init>"));
        self.alloc(
            TreeKind::Select(Select {
                qualifier: function,
                name: *name.as_name(),
                backquoted: false,
            }),
            position,
        )
    }

    pub(crate) fn parse_type_application(
        &mut self,
        mark: crate::Mark,
        function: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        let args = self.parse_type_argument_list(false);
        self.alloc_from(
            mark,
            TreeKind::TypeApply(dotty_core::ast::TypeApply { function, args }),
        )
    }

    pub(super) fn simple_expr_rest(
        &mut self,
        mark: crate::Mark,
        mut qualifier: TreeId<Untyped>,
        mut can_apply: bool,
    ) -> TreeId<Untyped> {
        loop {
            let checkpoint = self.cursor.checkpoint();
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
                can_apply = true;
            } else if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftBracket))
            {
                qualifier = self.parse_type_application(mark, qualifier);
                can_apply = true;
            } else if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftParen))
            {
                if !can_apply {
                    let message = if matches!(self.ast.get(qualifier).kind, TreeKind::Block(_)) {
                        "a block expression cannot be applied as a function"
                    } else {
                        "a constructor application cannot be applied again"
                    };
                    self.report(crate::ParseDiagnosticKind::UnexpectedToken, message);
                    break;
                }
                let is_constructor_application =
                    matches!(self.ast.get(qualifier).kind, TreeKind::New(_));
                qualifier = self.parse_application(mark, qualifier);
                if is_constructor_application {
                    can_apply = false;
                }
            } else {
                break;
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing an expression suffix",
                );
                break;
            }
        }
        qualifier
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

        let first = self.with_location(crate::Location::InParens, |parser| parser.expr());
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
            elements.push(self.with_location(crate::Location::InParens, |parser| parser.expr()));
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
            "expected an expression",
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
    use dotty_core::NameInterner;
    use dotty_core::ast::{Ident, ValDef};

    #[test]
    fn parses_an_expression_placeholder_as_a_synthetic_identifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "_",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        let TreeKind::Ident(Ident { name, backquoted }) = parser.ast().get(tree).kind else {
            panic!("expected a placeholder identifier");
        };
        assert!(!backquoted);
        assert_eq!(parser.placeholder_params.len(), 1);
        let parameter = parser.placeholder_params[0];
        assert!(matches!(
            parser.ast().get(parameter).kind,
            TreeKind::ValDef(ValDef { rhs: None, .. })
        ));
        assert_eq!(
            parser.ast().get(tree).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
        assert_eq!(
            parser.ast().get(parameter).position.unwrap().span().range(),
            TextRange::new(0, 0).unwrap()
        );
        assert_eq!(
            name,
            match &parser.ast().get(parameter).kind {
                TreeKind::ValDef(definition) => *definition.name.as_name(),
                _ => unreachable!(),
            }
        );
    }

    #[test]
    fn keeps_backquoted_underscore_as_a_regular_identifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "`_`",
            vec![
                token(TokenKind::BackquotedIdentifier, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::Ident(Ident {
                backquoted: true,
                ..
            })
        ));
        assert!(parser.placeholder_params.is_empty());
    }
}
