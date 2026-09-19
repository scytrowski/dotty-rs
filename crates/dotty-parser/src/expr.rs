use dotty_core::ast::New;
use dotty_core::ast::{
    Apply, ApplyKind, Block, Ident, Parens, Select, Super, This, Tuple, UntypedNode,
};
use dotty_core::{
    Constant, Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped,
};

use crate::Parser;

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
            TokenKind::Keyword(dotty_core::HardKeyword::Super) => self.parse_super(mark, None),
            TokenKind::Keyword(dotty_core::HardKeyword::New) => self.parse_new(mark),
            TokenKind::Punctuation(Punctuation::LeftParen) => self.parse_parens_or_tuple(mark),
            TokenKind::Punctuation(Punctuation::LeftBrace) => self.parse_block(mark),
            _ => self.unexpected_expression(),
        }
    }

    fn parse_block(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let mut trees = Vec::new();
        self.consume_block_separators();

        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Eof
        ) {
            let checkpoint = self.cursor.checkpoint();
            trees.push(self.simple_expr());

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a block",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if is_block_separator(self.current().kind) {
                self.consume_block_separators();
            } else if !matches!(
                self.current().kind,
                TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Eof
            ) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "expected a block statement separator",
                );
                self.recover_until(crate::RecoverySet::Statement);
                self.consume_block_separators();
            }
        }

        let expr = match trees.pop() {
            Some(expr) => expr,
            None => self.synthetic_unit(),
        };
        if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected `}` to close block",
            );
        }
        self.alloc_from(mark, TreeKind::Block(Block { stats: trees, expr }))
    }

    fn consume_block_separators(&mut self) {
        while is_block_separator(self.current().kind) {
            self.advance();
        }
    }

    fn synthetic_unit(&mut self) -> TreeId<Untyped> {
        let start = self.current().span.start();
        let range = TextRange::new(start, start).expect("zero-width synthetic unit range");
        self.alloc(
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Unit,
            }),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }

    fn parse_qualified_super(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
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

    fn parse_super(
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

    fn parse_super_tail(
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

    fn constructor_select(&mut self, function: TreeId<Untyped>) -> TreeId<Untyped> {
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

    fn parse_type_application(
        &mut self,
        mark: crate::Mark,
        function: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        self.advance();
        let mut args = Vec::new();
        if self.accept(TokenKind::Punctuation(Punctuation::RightBracket)) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedType,
                "expected a type argument between `[` and `]`",
            );
        } else {
            loop {
                args.push(self.simple_type());
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
                    break;
                }
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightBracket))
                {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedType,
                        "expected a type argument after `,`",
                    );
                    self.advance();
                    break;
                }
            }
        }
        self.alloc_from(
            mark,
            TreeKind::TypeApply(dotty_core::ast::TypeApply { function, args }),
        )
    }

    fn simple_expr_rest(
        &mut self,
        mark: crate::Mark,
        mut qualifier: TreeId<Untyped>,
        mut can_apply: bool,
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
                    self.report(
                        crate::ParseDiagnosticKind::UnexpectedToken,
                        "a block expression cannot be applied as a function",
                    );
                    break;
                }
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
        let function = if matches!(self.ast.get(function).kind, TreeKind::New(_)) {
            self.constructor_select(function)
        } else {
            function
        };
        let mut args = Vec::new();
        if !self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            loop {
                args.push(self.simple_expr());
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

        let first = self.simple_expr();
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
            elements.push(self.simple_expr());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Literal, New, Parens, Super, This, Tuple, UntypedNode};
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

        let id = parser.simple_expr();
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

        let id = parser.simple_expr();
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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected selection tree");
        };

        assert!(selection.backquoted);
        assert_eq!(names.resolve(selection.name.text()), "bar");
    }

    #[test]
    fn parses_super_selection_with_a_source_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super.foo",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected super selection tree");
        };
        let TreeKind::Super(Super { qual, mix }) = parser.ast().get(selection.qualifier).kind
        else {
            panic!("expected super qualifier");
        };

        assert!(mix.is_none());
        assert!(matches!(
            parser.ast().get(qual).kind,
            TreeKind::This(This { qual: None })
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 9).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        let selected_name = selection.name;
        drop(parser);
        assert_eq!(names.resolve(selected_name.text()), "foo");
    }

    #[test]
    fn parses_qualified_super_with_a_mixin_qualifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Outer.super[Base].foo",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Keyword(HardKeyword::Super), 6, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 11, 12),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                token(TokenKind::Punctuation(Punctuation::Dot), 17, 18),
                token(TokenKind::Identifier, 18, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected qualified super selection tree");
        };
        let TreeKind::Super(Super { qual, mix }) = parser.ast().get(selection.qualifier).kind
        else {
            panic!("expected qualified super tree");
        };
        let TreeKind::This(This { qual: Some(outer) }) = parser.ast().get(qual).kind else {
            panic!("expected qualified this tree");
        };

        assert!(outer.is_type());
        assert!(parser.diagnostics().is_empty());
        let outer_name = outer;
        let mix_name = mix.unwrap();
        let selected_name = selection.name;
        drop(parser);
        assert_eq!(names.resolve(outer_name.text()), "Outer");
        assert_eq!(names.resolve(mix_name.text()), "Base");
        assert_eq!(names.resolve(selected_name.text()), "foo");
    }

    #[test]
    fn rejects_super_without_a_selector() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_qualified_super_without_a_selector() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Outer.super",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Keyword(HardKeyword::Super), 6, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_mixin_qualified_super_without_a_selector() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super[Base]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_new_with_a_simple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "new Foo",
            vec![
                token(TokenKind::Keyword(HardKeyword::New), 0, 3),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected implicit constructor application");
        };
        let TreeKind::Select(selection) = parser.ast().get(application.function).kind else {
            panic!("expected constructor selection");
        };
        let init_name = selection.name;
        let TreeKind::New(New { tpt }) = parser.ast().get(selection.qualifier).kind else {
            panic!("expected new tree");
        };
        let TreeKind::Ident(ident) = parser.ast().get(tpt).kind else {
            panic!("expected constructed type");
        };

        assert!(ident.name.is_type());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 7).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(init_name.text()), "<init>");
    }

    #[test]
    fn parses_new_qualified_type_and_constructor_application() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "new foo.Bar(1)",
            vec![
                token(TokenKind::Keyword(HardKeyword::New), 0, 3),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Punctuation(Punctuation::Dot), 7, 8),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 11, 12),
                token(TokenKind::IntegerLiteral, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Apply(ref application) = parser.ast().get(id).kind else {
            panic!("expected constructor application");
        };
        let TreeKind::Select(selection) = parser.ast().get(application.function).kind else {
            panic!("expected constructor selection");
        };
        let TreeKind::New(New { tpt }) = parser.ast().get(selection.qualifier).kind else {
            panic!("expected new function");
        };
        let TreeKind::Select(selection) = parser.ast().get(tpt).kind else {
            panic!("expected qualified constructed type");
        };

        assert!(selection.name.is_type());
        assert_eq!(application.args.len(), 1);
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 14).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_type_application_with_multiple_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo[A, B]",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::TypeApply(type_apply) = &parser.ast().get(id).kind else {
            panic!("expected type application");
        };

        assert_eq!(type_apply.args.len(), 2);
        assert!(type_apply
            .args
            .iter()
            .all(|arg| matches!(parser.ast().get(*arg).kind, TreeKind::Ident(ident) if ident.name.is_type())));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 9).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn chains_type_application_application_and_selection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo[A](1).bar",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::IntegerLiteral, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::Dot), 9, 10),
                token(TokenKind::Identifier, 10, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Select(selection) = &parser.ast().get(id).kind else {
            panic!("expected final selection");
        };
        let TreeKind::Apply(application) = &parser.ast().get(selection.qualifier).kind else {
            panic!("expected application before selection");
        };
        assert!(matches!(
            parser.ast().get(application.function).kind,
            TreeKind::TypeApply(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 13).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_type_application_on_new_before_constructor_application() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "new Foo[Int](1)",
            vec![
                token(TokenKind::Keyword(HardKeyword::New), 0, 3),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 7, 8),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 12, 13),
                token(TokenKind::IntegerLiteral, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected constructor application");
        };
        let TreeKind::Select(selection) = parser.ast().get(application.function).kind else {
            panic!("expected constructor selection");
        };
        let TreeKind::New(New { tpt }) = parser.ast().get(selection.qualifier).kind else {
            panic!("expected new tree");
        };
        assert!(matches!(parser.ast().get(tpt).kind, TreeKind::TypeApply(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_empty_block_as_a_unit_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{}",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
            panic!("expected block tree");
        };

        assert!(stats.is_empty());
        assert!(matches!(
            parser.ast().get(expr).kind,
            TreeKind::Literal(Literal {
                value: Constant::Unit
            })
        ));
        assert_eq!(
            parser.ast().get(expr).position.unwrap().span().range(),
            TextRange::new(1, 1).unwrap()
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 2).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_block_with_one_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ x }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
            panic!("expected block tree");
        };

        assert!(stats.is_empty());
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_block_statements_and_keeps_the_last_as_expr() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ x; y }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
            panic!("expected block tree");
        };

        assert_eq!(stats.len(), 1);
        assert!(matches!(
            parser.ast().get(stats[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_multiline_block_statements() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{\n x\n y\n}",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Newline, 4, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Block(Block { ref stats, expr }) = parser.ast().get(id).kind else {
            panic!("expected block tree");
        };

        assert_eq!(stats.len(), 1);
        assert!(matches!(
            parser.ast().get(stats[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_application_of_a_block_expression() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "{ x }(y)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 4, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 5, 6),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        let TreeKind::Block(Block { expr, .. }) = result.ast.get(result.root).kind else {
            panic!("expected compilation-unit block root");
        };

        assert!(matches!(result.ast.get(expr).kind, TreeKind::Block(_)));
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn allows_application_after_selecting_from_a_block_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ f }.foo(1)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 4, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::IntegerLiteral, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert_eq!(application.args.len(), 1);
        let TreeKind::Select(selection) = &parser.ast().get(application.function).kind else {
            panic!("expected selection tree");
        };
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_an_incomplete_new_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "new",
            vec![
                token(TokenKind::Keyword(HardKeyword::New), 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Apply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_an_incomplete_type_application() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo[",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::TypeApply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_type_application_without_a_closing_bracket() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo[A",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::TypeApply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_block_without_a_closing_brace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ x",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Block(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_an_application_without_a_closing_parenthesis() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo(",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Apply(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_super_without_a_type_qualifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "super[",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Super(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

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

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
    }
}
