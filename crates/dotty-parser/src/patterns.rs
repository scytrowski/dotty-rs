use dotty_core::ast::{
    Alternative, Apply, ApplyKind, Bind, Ident, NamedArg, Parens, Select, Tuple, TypedExpr,
    UntypedNode,
};
use dotty_core::{
    Constant, HardKeyword, Punctuation, SourceId, SourceSpan, SourceText, Span, TextRange,
    TokenKind, TokenSource, TreeId, TreeKind, Untyped,
};

use crate::{Location, ParseDiagnosticKind, ParseKind, ParseResult, Parser};

/// Parses one source-level pattern fragment with the same pattern grammar used
/// by future case clauses and generators.
pub fn parse_pattern_fragment<S: TokenSource>(
    source: SourceText<'_>,
    source_id: SourceId,
    tokens: S,
    names: &mut dotty_core::NameInterner,
) -> ParseResult {
    Parser::new(source, source_id, tokens, names).parse_pattern_fragment()
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: TokenSource,
{
    /// Parses a standalone pattern for parser tooling and differential tests.
    /// This is a fragment entry, not a second parser dialect.
    pub fn parse_pattern_fragment(mut self) -> ParseResult {
        let pattern = self.with_parse_kind(ParseKind::Pattern, |parser| {
            parser.with_location(Location::InPattern, |parser| parser.pattern())
        });

        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
        if self.current().kind != TokenKind::Eof {
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
                "expected end of pattern fragment",
            );
            self.recover_until(crate::RecoverySet::Statement);
        }

        ParseResult {
            ast: self.ast,
            root: pattern,
            diagnostics: self.diagnostics,
        }
    }

    pub(crate) fn pattern(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let first = self.pattern1();
        if !self.current_text_is("|") {
            return first;
        }

        let mut alternatives = vec![first];
        while self.current_text_is("|") {
            self.advance();
            if !can_start_simple_pattern(self) {
                self.report(
                    ParseDiagnosticKind::ExpectedPattern,
                    "expected a pattern after `|`",
                );
                alternatives.push(self.error_pattern(self.current_span()));
                break;
            }
            alternatives.push(self.pattern1());
        }

        self.alloc_from(mark, TreeKind::Alternative(Alternative { alternatives }))
    }

    fn pattern1(&mut self) -> TreeId<Untyped> {
        let pattern = self.pattern2();
        if !self.current_is_pattern_colon() {
            return pattern;
        }

        let start = self
            .ast()
            .get(pattern)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start);
        self.advance();
        let tpt = self.simple_type();
        let range = TextRange::new(start, self.last_real_token_end)
            .expect("typed pattern span endpoints are ordered");
        self.alloc(
            TreeKind::Typed(TypedExpr { expr: pattern, tpt }),
            Some(SourceSpan::new(
                self.source_id(),
                Span::without_point(range),
            )),
        )
    }

    fn pattern2(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let pattern = self.infix_pattern();
        if !self.current_text_is("@") {
            return pattern;
        }

        let TreeKind::Ident(identifier) = self.ast().get(pattern).kind else {
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
                "a pattern binder must start with an identifier",
            );
            self.advance();
            return pattern;
        };
        self.advance();
        let body = self.pattern3();
        self.alloc_from(
            mark,
            TreeKind::Bind(Bind {
                name: identifier.name,
                body,
            }),
        )
    }

    fn pattern3(&mut self) -> TreeId<Untyped> {
        let pattern = self.infix_pattern();
        if self.current_text_is("*") {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "sequence patterns are not supported yet",
            );
        }
        pattern
    }

    fn infix_pattern(&mut self) -> TreeId<Untyped> {
        let mut operands = vec![self.simple_pattern()];
        let mut operators: Vec<dotty_core::Name> = Vec::new();

        while let Some(operator) = self.current_pattern_operator() {
            let checkpoint = self.cursor.checkpoint();
            let spelling = self.names.resolve(operator.text()).to_owned();
            let precedence = crate::precedence(&spelling);
            let left_associative = !crate::is_right_associative(&spelling);
            if !can_start_simple_pattern_kind(self.cursor.lookahead(1).kind) {
                break;
            }

            while let Some(top) = operators.last().copied() {
                let top_spelling = self.names.resolve(top.text());
                let top_precedence = crate::precedence(top_spelling);
                if !(precedence < top_precedence
                    || (left_associative && precedence == top_precedence))
                {
                    break;
                }
                let top = operators
                    .pop()
                    .expect("pattern operator stack is non-empty");
                let right = operands.pop().expect("pattern operand stack is non-empty");
                let left = operands.pop().expect("pattern operand stack is non-empty");
                operands.push(self.alloc_infix(left, top, right));
            }

            self.advance();
            self.consume_pattern_newlines();
            operators.push(operator);
            operands.push(self.simple_pattern());
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing an infix pattern",
                );
                break;
            }
        }

        while let Some(operator) = operators.pop() {
            let right = operands.pop().expect("pattern operand stack is non-empty");
            let left = operands.pop().expect("pattern operand stack is non-empty");
            operands.push(self.alloc_infix(left, operator, right));
        }
        operands
            .pop()
            .expect("pattern parser always allocates an initial operand")
    }

    fn simple_pattern(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let tree = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return self.error_pattern(self.current_span());
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
            TokenKind::Operator if self.current_text_is("-") => {
                if !is_numeric_literal(self.cursor.lookahead(1).kind) {
                    return self.unexpected_pattern();
                }
                self.advance();
                self.parse_negative_number(mark)
            }
            TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral => self.parse_number(mark),
            TokenKind::StringLiteral => self.parse_string(mark),
            TokenKind::Keyword(HardKeyword::True) => {
                self.parse_literal(mark, Constant::Boolean(true))
            }
            TokenKind::Keyword(HardKeyword::False) => {
                self.parse_literal(mark, Constant::Boolean(false))
            }
            TokenKind::Keyword(HardKeyword::Null) => self.parse_literal(mark, Constant::Null),
            TokenKind::Keyword(HardKeyword::This) => {
                self.advance();
                self.alloc_from(mark, TreeKind::This(dotty_core::ast::This { qual: None }))
            }
            TokenKind::Punctuation(Punctuation::LeftParen) => self.parse_pattern_parens(mark),
            _ => self.unexpected_pattern(),
        };

        if matches!(self.ast().get(tree).kind, TreeKind::Ident(_)) {
            self.simple_pattern_rest(mark, tree)
        } else {
            tree
        }
    }

    fn simple_pattern_rest(
        &mut self,
        mark: crate::Mark,
        mut tree: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        loop {
            if self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return tree;
                };
                self.advance();
                tree = self.alloc_from(
                    mark,
                    TreeKind::Select(Select {
                        qualifier: tree,
                        name: *name.as_name(),
                        backquoted,
                    }),
                );
            } else if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftBracket))
            {
                tree = self.parse_type_application(mark, tree);
            } else if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::LeftParen))
            {
                tree = self.parse_argument_patterns(mark, tree);
            } else {
                break;
            }
        }
        tree
    }

    fn parse_pattern_parens(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple {
                    elements: Vec::new(),
                })),
            );
        }

        let first = self.with_location(Location::InParens, |parser| parser.pattern());
        if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner: first })),
            );
        }

        let mut elements = vec![first];
        while !self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::RightParen))
            && self.current().kind != TokenKind::Eof
        {
            elements.push(self.with_location(Location::InParens, |parser| parser.pattern()));
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

    fn parse_argument_patterns(
        &mut self,
        mark: crate::Mark,
        function: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        self.advance();
        let mut args = Vec::new();
        if !self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            loop {
                args.push(
                    self.with_location(Location::InPatternArgs, |parser| parser.pattern_argument()),
                );
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        ParseDiagnosticKind::ExpectedPattern,
                        "expected a pattern after `,`",
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

    fn pattern_argument(&mut self) -> TreeId<Untyped> {
        if is_identifier_kind(self.current().kind) && self.token_text_at(1) == Some("=") {
            let name = match self.intern_current_term_name() {
                Ok(name) => *name.as_name(),
                Err(_) => return self.unexpected_pattern(),
            };
            let mark = self.mark();
            self.advance();
            self.advance();
            let arg = self.pattern();
            return self.alloc_from(mark, TreeKind::NamedArg(NamedArg { name, arg }));
        }
        self.pattern()
    }

    fn consume_pattern_newlines(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn current_pattern_operator(&mut self) -> Option<dotty_core::Name> {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::Operator
                | TokenKind::ColonOp
        ) {
            return None;
        }
        let spelling = self.current_text().ok()?;
        if matches!(spelling, "|" | "@" | "=") {
            return None;
        }
        if !can_start_simple_pattern_kind(self.cursor.lookahead(1).kind) {
            return None;
        }
        Some(*self.intern_current_term_name().ok()?.as_name())
    }

    fn current_is_pattern_colon(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::ColonOp
                | TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::Punctuation(Punctuation::Colon)
        ) && self.current_text().ok() == Some(":")
    }

    fn current_text_is(&self, expected: &str) -> bool {
        self.current_text().ok() == Some(expected)
    }

    fn token_text_at(&mut self, offset: usize) -> Option<&'src str> {
        let token = self.cursor.lookahead(offset);
        self.source.slice(token.span).ok()
    }

    fn unexpected_pattern(&mut self) -> TreeId<Untyped> {
        let position = self.current_span();
        self.report(ParseDiagnosticKind::ExpectedPattern, "expected a pattern");
        if self.current().kind != TokenKind::Eof {
            self.advance();
        }
        self.error_pattern(position)
    }
}

fn is_identifier_kind(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier | TokenKind::BackquotedIdentifier
    )
}

fn is_numeric_literal(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IntegerLiteral
            | TokenKind::LongLiteral
            | TokenKind::DecimalLiteral
            | TokenKind::ExponentLiteral
            | TokenKind::FloatLiteral
            | TokenKind::DoubleLiteral
    )
}

fn can_start_simple_pattern_kind(kind: TokenKind) -> bool {
    is_identifier_kind(kind)
        || is_numeric_literal(kind)
        || matches!(
            kind,
            TokenKind::StringLiteral
                | TokenKind::Keyword(HardKeyword::True)
                | TokenKind::Keyword(HardKeyword::False)
                | TokenKind::Keyword(HardKeyword::Null)
                | TokenKind::Keyword(HardKeyword::This)
                | TokenKind::Punctuation(Punctuation::LeftParen)
        )
}

fn can_start_simple_pattern<S: TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    can_start_simple_pattern_kind(parser.current().kind)
        || (parser.current().kind == TokenKind::Operator
            && parser.current_text().ok() == Some("-")
            && is_numeric_literal(parser.cursor.lookahead(1).kind))
}
