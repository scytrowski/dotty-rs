use dotty_core::ast::{
    Apply, ApplyKind, Block, Ident, NamedArg, New, Parens, Select, Super, This, Tuple, UntypedNode,
};
use dotty_core::{
    Constant, Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped,
};

use super::is_block_separator;
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
        self.with_block_end(Some(end), |parser| {
            parser.parse_expression_block_body_inner(end)
        })
    }

    fn parse_expression_block_body_inner(
        &mut self,
        end: TokenKind,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        let mut trees = Vec::new();
        self.consume_block_separators(end);

        while !self.expression_block_body_ended(end) {
            let checkpoint = self.cursor.checkpoint();
            trees.push(self.with_location(crate::Location::InBlock, |parser| parser.expr()));

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
                self.consume_block_separators(end);
            } else if !self.expression_block_body_ended(end) {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "expected a block statement separator",
                );
                self.recover_until(crate::RecoverySet::Statement);
                self.consume_block_separators(end);
            }
        }

        let expr = match trees.pop() {
            Some(expr) => expr,
            None => self.synthetic_unit(),
        };
        (trees, expr)
    }

    fn expression_block_body_ended(&self, end: TokenKind) -> bool {
        self.current().kind == end
            || self.current().kind == TokenKind::Eof
            || (self.context.case_body && self.is_case_body_terminator())
    }

    fn consume_block_separators(&mut self, end: TokenKind) {
        while is_block_separator(self.current().kind)
            && self.current().kind != end
            && !(self.context.case_body && self.is_case_body_terminator())
        {
            self.advance();
        }
    }

    fn is_case_body_terminator(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Keyword(dotty_core::HardKeyword::Case)
                | TokenKind::Punctuation(Punctuation::RightBrace)
                | TokenKind::Outdent
        )
    }

    pub(super) fn synthetic_unit(&mut self) -> TreeId<Untyped> {
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

    pub(crate) fn parse_type_application(
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
                args.push(self.argument_expr());
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

    fn argument_expr(&mut self) -> TreeId<Untyped> {
        let tree = self.with_location(crate::Location::InArgs, |parser| parser.expr());
        self.normalize_named_argument(tree)
    }

    fn normalize_named_argument(&mut self, tree: TreeId<Untyped>) -> TreeId<Untyped> {
        let TreeKind::Assign(assignment) = self.ast.get(tree).kind else {
            return tree;
        };
        let TreeKind::Ident(identifier) = self.ast.get(assignment.lhs).kind else {
            return tree;
        };

        self.alloc(
            TreeKind::NamedArg(NamedArg {
                name: identifier.name,
                arg: assignment.rhs,
            }),
            self.ast.get(tree).position,
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
