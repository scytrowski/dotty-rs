use dotty_core::ast::{
    Apply, ApplyKind, Block, Ident, New, Parens, Quote, Select, SplicePattern, Super, This, Tuple,
    UntypedNode, ValDef,
};
use dotty_core::{
    Constant, Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped,
};

use crate::Parser;
use crate::statements::{ParsedStatement, StatementSequenceBoundary};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses one simple expression and all of its currently supported suffixes.
    pub(crate) fn simple_expr(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        // Constructor argument clauses belong to `new` itself. In particular,
        // an anonymous template body is a completed `NEW` and cannot acquire
        // another application suffix in `simple_expr_rest`.
        let can_apply = self.current().kind != TokenKind::Keyword(dotty_core::HardKeyword::New);
        let tree = self.simple_expr_atom(mark);
        let can_apply = can_apply
            && !matches!(
                self.ast.get(tree).kind,
                TreeKind::Block(_) | TreeKind::Match(_)
            );

        self.simple_expr_rest(mark, tree, can_apply)
    }

    /// Parses only the parenthesized condition when legacy if/while syntax
    /// leaves the following expression to be parsed as the body.
    pub(super) fn parenthesized_condition_atom(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        self.parse_parens_or_tuple(mark)
    }

    fn simple_expr_atom(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        if self.expression_quote_depth > 0
            && (self.current_starts_braced_splice() || self.current_starts_simple_splice())
        {
            return self.parse_expression_splice(mark);
        }

        if self.current_operator_is_term_identifier() {
            let Ok(name) = self.intern_current_term_name() else {
                return self.unexpected_expression();
            };
            self.advance();
            return self.alloc_from(
                mark,
                TreeKind::Ident(Ident {
                    name: *name.as_name(),
                    backquoted: false,
                }),
            );
        }

        if let Some(tree) = self.parse_qualified_this_reference(mark) {
            return tree;
        }

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
            TokenKind::CharLiteral => self.parse_char(mark),
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
            TokenKind::Indent => self.parse_indented_block(),
            _ => self.unexpected_expression(),
        }
    }

    fn current_operator_is_term_identifier(&mut self) -> bool {
        if self.current().kind != TokenKind::Operator {
            return false;
        }

        self.current_text()
            .ok()
            .is_some_and(is_term_operator_identifier)
    }

    fn parse_quote(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let in_pattern = self.context.parse_kind == crate::ParseKind::Pattern;
        self.advance();
        match self.current().kind {
            TokenKind::Punctuation(Punctuation::LeftBrace) => {
                self.advance();
                let body_mark = self.mark();
                self.expression_quote_depth += 1;
                self.quote_pattern_depth += u32::from(in_pattern);
                let (stats, expr) = self
                    .parse_expression_block_body(TokenKind::Punctuation(Punctuation::RightBrace));
                self.quote_pattern_depth -= u32::from(in_pattern);
                self.expression_quote_depth -= 1;
                let body = if stats.is_empty() {
                    if self.is_synthetic_unit(expr) {
                        self.alloc(
                            TreeKind::Block(Block { stats, expr }),
                            Some(self.zero_width_span(mark.start())),
                        )
                    } else {
                        expr
                    }
                } else {
                    let first = stats[0];
                    let start = self
                        .ast
                        .get(first)
                        .position
                        .map(|position| position.span().range().start())
                        .unwrap_or(body_mark.start());
                    self.alloc_from(
                        crate::Mark { start },
                        TreeKind::Block(Block { stats, expr }),
                    )
                };
                if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedToken,
                        "expected `}` to close quoted expression",
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
            TokenKind::Punctuation(Punctuation::LeftBracket) => {
                self.advance();
                self.type_quote_depth += 1;
                let body = self.with_parse_kind(crate::ParseKind::Type, |parser| {
                    parser.parse_type_quote_body()
                });
                self.type_quote_depth -= 1;
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

    /// Parses Dotty's `TypeBlock ::= { TypeBlockStat semi } Type` used by
    /// quoted types. A type block with local aliases is represented by the
    /// shared `Block` node, just like Dotty's source tree.
    fn parse_type_quote_body(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut stats = Vec::new();

        while self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Type) {
            let checkpoint = self.cursor.checkpoint();
            if let ParsedStatement::Definition(definition) =
                self.parse_type_definition(crate::Location::InBlock)
            {
                stats.push(definition);
            }

            if !self.cursor.progressed_since(checkpoint) {
                break;
            }

            let definition_end = stats
                .last()
                .and_then(|definition| self.ast.get(*definition).position)
                .map(|position| position.span().range().end());
            let mut consumed_separator = false;
            while matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Punctuation(Punctuation::Semicolon)
                    | TokenKind::Indent
                    | TokenKind::Outdent
            ) {
                consumed_separator = true;
                self.advance();
            }

            if !consumed_separator
                && self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Type)
                && let Some(definition_end) = definition_end
            {
                let separator_range =
                    TextRange::new(definition_end, self.current().span.start()).ok();
                consumed_separator = separator_range
                    .and_then(|range| self.source.slice(range).ok())
                    .is_some_and(|trivia| {
                        trivia.contains('\n') || trivia.contains('\r') || trivia.contains(';')
                    });
            }

            if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Type)
                && !consumed_separator
            {
                self.report(
                    crate::ParseDiagnosticKind::UnexpectedToken,
                    "expected a separator between quoted type definitions",
                );
                break;
            }
        }

        let expr = self.type_expr();
        if stats.is_empty() {
            expr
        } else {
            self.alloc_from(mark, TreeKind::Block(Block { stats, expr }))
        }
    }

    fn parse_expression_splice(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        if self.current_starts_simple_splice() {
            let token = self.current().clone();
            let spelling = self
                .token_text(&token)
                .ok()
                .and_then(|text| text.strip_prefix('$'))
                .map(str::to_owned);
            let Some(spelling) = spelling else {
                return self.unexpected_expression();
            };
            let name = self.names.intern(&spelling);
            self.advance();
            let expr = self.alloc_from(
                crate::Mark {
                    start: token.span.start() + 1,
                },
                TreeKind::Ident(Ident {
                    name: *dotty_core::TermName::new(name).as_name(),
                    backquoted: false,
                }),
            );
            if self.quote_pattern_depth > 0 {
                return self.alloc_from(
                    mark,
                    TreeKind::SplicePattern(SplicePattern {
                        body: expr,
                        type_args: Vec::new(),
                        args: Vec::new(),
                    }),
                );
            }
            return self.alloc_from(mark, TreeKind::Splice(dotty_core::ast::Splice { expr }));
        }

        self.advance(); // `$`
        self.expect(TokenKind::Punctuation(Punctuation::LeftBrace));
        if self.quote_pattern_depth > 0 {
            let body = self.with_parse_kind(crate::ParseKind::Pattern, |parser| {
                parser.with_location(crate::Location::InPattern, |parser| parser.pattern())
            });
            if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `}` to close pattern splice",
                );
            }
            return self.alloc_from(
                mark,
                TreeKind::SplicePattern(SplicePattern {
                    body,
                    type_args: Vec::new(),
                    args: Vec::new(),
                }),
            );
        }

        let body_mark = self.mark();
        let (stats, expr) =
            self.parse_expression_block_body(TokenKind::Punctuation(Punctuation::RightBrace));
        let body = if stats.is_empty() {
            if self.is_synthetic_unit(expr) {
                self.alloc(
                    TreeKind::Block(Block { stats, expr }),
                    Some(self.zero_width_span(mark.start())),
                )
            } else {
                expr
            }
        } else {
            let first = stats[0];
            let start = self
                .ast
                .get(first)
                .position
                .map(|position| position.span().range().start())
                .unwrap_or(body_mark.start());
            self.alloc_from(
                crate::Mark { start },
                TreeKind::Block(Block { stats, expr }),
            )
        };
        if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected `}` to close expression splice",
            );
        }
        self.alloc_from(
            mark,
            TreeKind::Splice(dotty_core::ast::Splice { expr: body }),
        )
    }

    fn is_synthetic_unit(&self, id: TreeId<Untyped>) -> bool {
        matches!(
            &self.ast.get(id).kind,
            TreeKind::Literal(literal) if literal.value == Constant::Unit
        ) && self.ast.get(id).position.is_some_and(|position| {
            let range = position.span().range();
            range.start() == range.end()
        })
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
        if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Case) {
            let cases = self.case_clauses();
            if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                self.report(
                    crate::ParseDiagnosticKind::ExpectedToken,
                    "expected `}` to close case-lambda clauses",
                );
            }
            let selector = self.synthetic_unit_at(mark.start());
            return self.alloc_from(
                mark,
                TreeKind::Match(dotty_core::ast::Match { selector, cases }),
            );
        }
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
        self.parse_expression_block_body_with_boundary(StatementSequenceBoundary::Block(end))
    }

    pub(crate) fn parse_region_feedback_expression_block_body(
        &mut self,
        indent_offset: u32,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        self.parse_expression_block_body_with_boundary(
            StatementSequenceBoundary::FeedbackRegionBlock {
                closing: TokenKind::Outdent,
                indent_offset,
            },
        )
    }

    fn parse_expression_block_body_with_boundary(
        &mut self,
        boundary: StatementSequenceBoundary,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        self.with_enum_body(false, |parser| {
            parser.with_placeholder_scope(|parser| {
                parser.with_block_end(
                    match boundary {
                        StatementSequenceBoundary::Block(end)
                        | StatementSequenceBoundary::FeedbackRegionBlock { closing: end, .. } => {
                            Some(end)
                        }
                        StatementSequenceBoundary::CompilationUnit => None,
                    },
                    |parser| {
                        let feedback_indent = match boundary {
                            StatementSequenceBoundary::FeedbackRegionBlock {
                                indent_offset,
                                ..
                            } => Some(indent_offset),
                            StatementSequenceBoundary::Block(_)
                            | StatementSequenceBoundary::CompilationUnit => None,
                        };
                        parser.with_feedback_block_indent(feedback_indent, |parser| {
                            parser.parse_statement_sequence(boundary)
                        })
                    },
                )
            })
        })
    }

    pub(crate) fn synthetic_unit(&mut self) -> TreeId<Untyped> {
        self.synthetic_unit_at(self.current().span.start())
    }

    pub(crate) fn synthetic_unit_at(&mut self, start: u32) -> TreeId<Untyped> {
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

        let Some((name, backquoted)) = self.current_selector_name() else {
            self.report(
                crate::ParseDiagnosticKind::ExpectedToken,
                "expected a selector after `super.`",
            );
            return super_tree;
        };
        self.advance();
        self.alloc_from(
            mark,
            TreeKind::Select(Select {
                qualifier: super_tree,
                name,
                backquoted,
            }),
        )
    }

    fn parse_new(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.optional_template_body_starts_here() {
            let body = self.parse_optional_template_body();
            return self.new_with_anonymous_template(mark, Vec::new(), body);
        }

        // Dotty parses `new` parents as constructor applications separated by
        // `with`. Reuse the class-parent production for the same type and
        // constructor-argument shapes.
        let mut parents = vec![self.parse_parent()];
        while self.consume_new_parent_separator() {
            parents.push(self.parse_parent());
        }

        let has_template_body = self.optional_template_body_starts_here();
        if parents.len() > 1 || has_template_body {
            let body = if has_template_body {
                self.parse_optional_template_body()
            } else {
                crate::templates::TemplateBodyResult {
                    self_val: None,
                    members: Vec::new(),
                }
            };
            self.new_with_anonymous_template(mark, parents, body)
        } else {
            let parent = parents[0];
            if matches!(self.ast.get(parent).kind, TreeKind::Apply(_)) {
                if let Some(position) = self.ast.get(parent).position {
                    let end = position.span().range().end();
                    let range =
                        TextRange::new(mark.start, end).expect("new expression span is ordered");
                    self.ast.get_mut(parent).position =
                        Some(SourceSpan::new(self.source_id, Span::without_point(range)));
                }
                parent
            } else {
                let constructor = self.alloc(
                    TreeKind::New(New { tpt: parent }),
                    self.ast.get(parent).position,
                );
                let init = self.constructor_select(constructor);
                self.alloc_from(
                    mark,
                    TreeKind::Apply(Apply {
                        function: init,
                        args: Vec::new(),
                        kind: ApplyKind::Regular,
                    }),
                )
            }
        }
    }

    fn consume_new_parent_separator(&mut self) -> bool {
        let mut newlines = 0;
        while matches!(
            self.cursor.lookahead(newlines).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            newlines += 1;
        }
        if self.cursor.lookahead(newlines).kind != TokenKind::Keyword(dotty_core::HardKeyword::With)
        {
            return false;
        }
        for _ in 0..newlines {
            self.advance();
        }
        self.advance();
        true
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
        qualifier: TreeId<Untyped>,
        can_apply: bool,
    ) -> TreeId<Untyped> {
        self.simple_expr_rest_with_brace_application(mark, qualifier, can_apply, true)
    }

    fn simple_expr_rest_with_brace_application(
        &mut self,
        mark: crate::Mark,
        mut qualifier: TreeId<Untyped>,
        mut can_apply: bool,
        allow_brace_application: bool,
    ) -> TreeId<Untyped> {
        loop {
            if self.cursor.at(TokenKind::Punctuation(Punctuation::Dot))
                && self
                    .has_physical_line_break(self.last_real_token_end, self.current().span.start())
            {
                // A leading selector continues an expression unless it
                // dedents out of an active layout body. Let the scanner close
                // that body before deciding whether the dot is a suffix.
                self.observe_outdented();
            }
            let checkpoint = self.cursor.checkpoint();
            if self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Match) {
                    qualifier = self.parse_match_clause(qualifier);
                } else if let Some((name, backquoted)) = self.current_selector_name() {
                    self.advance();
                    qualifier = self.alloc_from(
                        mark,
                        TreeKind::Select(Select {
                            qualifier,
                            name,
                            backquoted,
                        }),
                    );
                } else {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedToken,
                        "expected a selector after `.`",
                    );
                    return qualifier;
                }
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
                    let message = match &self.ast.get(qualifier).kind {
                        TreeKind::Block(_) => "a block expression cannot be applied as a function",
                        TreeKind::Match(_) => "a case-lambda cannot be applied directly",
                        _ => "a constructor application cannot be applied again",
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
            } else if self.optional_template_body_starts_here()
                && let Some(parent) = self.new_template_parent(qualifier)
            {
                let body = self.parse_optional_template_body();
                qualifier = self.new_with_anonymous_template(mark, vec![parent], body);
                can_apply = false;
            } else if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftBrace))
                && can_apply
                && allow_brace_application
            {
                let argument = self.parse_block(self.mark());
                qualifier = self.alloc_from(
                    mark,
                    TreeKind::Apply(Apply {
                        function: qualifier,
                        args: vec![argument],
                        kind: ApplyKind::Regular,
                    }),
                );
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

    fn new_template_parent(&mut self, tree: TreeId<Untyped>) -> Option<TreeId<Untyped>> {
        match &self.ast.get(tree).kind {
            TreeKind::New(new) => Some(new.tpt),
            TreeKind::Apply(application) => {
                let function = application.function;
                let constructor_select = match &self.ast.get(function).kind {
                    TreeKind::Select(selection)
                        if matches!(self.ast.get(selection.qualifier).kind, TreeKind::New(_)) =>
                    {
                        Some(function)
                    }
                    _ => self.new_constructor_select_in_application(function),
                }?;

                if let (Some(function_position), Some(parent_position)) = (
                    self.ast.get(constructor_select).position,
                    self.ast.get(tree).position,
                ) {
                    let start = function_position.span().range().start();
                    let end = parent_position.span().range().end();
                    let range = TextRange::new(start, end).expect("constructor span is ordered");
                    self.ast.get_mut(tree).position =
                        Some(SourceSpan::new(self.source_id, Span::without_point(range)));
                }
                Some(tree)
            }
            _ => None,
        }
    }

    fn new_constructor_select_in_application(
        &self,
        mut tree: TreeId<Untyped>,
    ) -> Option<TreeId<Untyped>> {
        loop {
            let TreeKind::Apply(application) = &self.ast.get(tree).kind else {
                return None;
            };
            let function = application.function;
            match &self.ast.get(function).kind {
                TreeKind::Select(selection)
                    if matches!(self.ast.get(selection.qualifier).kind, TreeKind::New(_)) =>
                {
                    return Some(function);
                }
                TreeKind::Apply(_) => tree = function,
                _ => return None,
            }
        }
    }

    fn new_with_anonymous_template(
        &mut self,
        mark: crate::Mark,
        parents: Vec<TreeId<Untyped>>,
        body: crate::templates::TemplateBodyResult,
    ) -> TreeId<Untyped> {
        let template = self.allocate_anonymous_new_template(mark.start, parents, body);
        self.ast.get_mut(template).position = Some(self.span_from(mark));
        self.alloc_from(mark, TreeKind::New(New { tpt: template }))
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

pub(super) fn is_term_operator_identifier(spelling: &str) -> bool {
    !matches!(spelling, "=" | "=>" | "=>>" | "?=>" | "<-" | "@" | "#")
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
