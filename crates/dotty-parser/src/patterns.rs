use dotty_core::ast::{
    Alternative, Apply, ApplyKind, Bind, Ident, NamedArg, Parens, Select, Tuple, TypedExpr,
    UntypedNode,
};
use dotty_core::{
    Constant, HardKeyword, Punctuation, SourceId, SourceSpan, SourceText, Span, TermName,
    TextRange, TokenKind, TokenSource, TreeId, TreeKind, Untyped,
};

use crate::{Location, ParseDiagnosticKind, ParseKind, ParseResult, Parser};

/// Parses one source-level pattern fragment with the same pattern grammar used
/// by case clauses and future generators.
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

        let effective_features = *self.features();
        ParseResult {
            ast: self.ast,
            root: pattern,
            diagnostics: self.diagnostics,
            effective_features,
        }
    }

    pub(crate) fn pattern(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let first = self.pattern1();
        let Some(mut pipe_offset) = self.pattern_alternative_operator_offset() else {
            return first;
        };

        let mut alternatives = vec![first];
        loop {
            if pipe_offset > 0 {
                self.consume_pattern_newlines();
            }
            self.advance();
            let Some(operand_offset) = pattern_alternative_operand_offset(self) else {
                self.report(
                    ParseDiagnosticKind::ExpectedPattern,
                    "expected a pattern after `|`",
                );
                alternatives.push(self.error_pattern(self.current_span()));
                break;
            };
            if operand_offset > 0 {
                self.consume_pattern_newlines();
            }
            let alternative = self.pattern1();
            alternatives.push(alternative);
            let Some(next_pipe_offset) = self.pattern_alternative_operator_offset() else {
                break;
            };
            pipe_offset = next_pipe_offset;
        }

        self.alloc_from(mark, TreeKind::Alternative(Alternative { alternatives }))
    }

    pub(crate) fn pattern1(&mut self) -> TreeId<Untyped> {
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
        let tpt = self.parse_refined_type();
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

    pub(crate) fn pattern2(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let pattern = self.pattern3(self.context.location);
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
        let binder_name = identifier.name;
        self.advance();
        let body = self.pattern3(self.context.location);

        // Dotty keeps a bound sequence wildcard as a typed binder rather than
        // nesting a Bind around the wildcard's Typed node: `x @ _*` becomes
        // `Typed(x, _*)`. Other binders retain the ordinary Bind shape.
        let wildcard_sequence_type = match &self.ast().get(body).kind {
            TreeKind::Typed(typed)
                if is_ident_named(self, typed.expr, "_")
                    && is_ident_named(self, typed.tpt, "_*") =>
            {
                Some(typed.tpt)
            }
            _ => None,
        };
        if let Some(tpt) = wildcard_sequence_type {
            return self.alloc_from(mark, TreeKind::Typed(TypedExpr { expr: pattern, tpt }));
        }

        self.alloc_from(
            mark,
            TreeKind::Bind(Bind {
                name: binder_name,
                body,
                given: false,
            }),
        )
    }

    fn pattern3(&mut self, location: Location) -> TreeId<Untyped> {
        let mark = self.mark();
        let pattern = self.infix_pattern();
        if !self.current_is_sequence_marker() {
            return pattern;
        }

        let following = self.cursor.lookahead(1).kind;
        let followed_by_pattern_delimiter = following
            == TokenKind::Punctuation(Punctuation::RightParen)
            || (following == TokenKind::Punctuation(Punctuation::Comma)
                && matches!(
                    self.cursor.lookahead(2).kind,
                    TokenKind::Punctuation(Punctuation::RightParen) | TokenKind::Eof
                ));
        let at_fragment_end = following == TokenKind::Eof;
        if !followed_by_pattern_delimiter && !at_fragment_end {
            return pattern;
        }

        let star_mark = self.mark();
        self.advance();
        if location == Location::InPatternArgs && is_pattern_variable(self, pattern) {
            let wildcard_star_name = *TermName::new(self.names.intern("_*")).as_name();
            let wildcard_star = self.alloc_from(
                star_mark,
                TreeKind::Ident(Ident {
                    name: wildcard_star_name,
                    backquoted: false,
                }),
            );
            return self.alloc_from(
                mark,
                TreeKind::Typed(TypedExpr {
                    expr: pattern,
                    tpt: wildcard_star,
                }),
            );
        }

        let message = if location == Location::InPatternArgs {
            "`*` must follow a pattern variable"
        } else {
            "sequence patterns are only allowed in extractor arguments"
        };
        self.report(ParseDiagnosticKind::UnsupportedSyntax, message);
        pattern
    }

    fn current_is_sequence_marker(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::Identifier
        ) && self.current_text_is("*")
    }

    fn infix_pattern(&mut self) -> TreeId<Untyped> {
        let mut operands = vec![self.simple_pattern()];
        let mut operators: Vec<dotty_core::Name> = Vec::new();

        while let Some(operator) = self.current_pattern_operator() {
            let checkpoint = self.cursor.checkpoint();
            let spelling = self.names.resolve(operator.text()).to_owned();
            let precedence = crate::precedence(&spelling);
            let left_associative = !crate::is_right_associative(&spelling);
            if self.pattern_operand_offset().is_none() {
                break;
            }

            if let Some(top) = operators.last().copied() {
                let top_spelling = self.names.resolve(top.text()).to_owned();
                if crate::infix::has_mixed_associativity(&top_spelling, &spelling) {
                    self.report(
                        ParseDiagnosticKind::UnexpectedToken,
                        format!(
                            "mixed left- and right-associative pattern operators `{top_spelling}` and `{spelling}`"
                        ),
                    );
                }
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
        let starts_qualified_this = self.current_starts_qualified_this();
        let starts_symbolic_extractor = self.current_starts_symbolic_pattern_extractor();
        let current_kind = self.current().kind;
        let tree = match current_kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier if starts_qualified_this => {
                self.parse_qualified_this_reference(mark)
                    .unwrap_or_else(|| self.error_pattern(self.current_span()))
            }
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
            TokenKind::Operator | TokenKind::ColonOp if starts_symbolic_extractor => {
                let Ok(name) = self.intern_current_term_name() else {
                    return self.error_pattern(self.current_span());
                };
                self.advance();
                self.alloc_from(
                    mark,
                    TreeKind::Ident(Ident {
                        name: *name.as_name(),
                        backquoted: false,
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
            TokenKind::InterpolationId => self.parse_interpolated_string_pattern(mark),
            TokenKind::CharLiteral => self.parse_char(mark),
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
            TokenKind::Keyword(HardKeyword::Super) => self.parse_super(mark, None),
            TokenKind::Quote => self.simple_expr(),
            TokenKind::Keyword(HardKeyword::Given) => self.parse_given_pattern(mark),
            TokenKind::XmlStart => self.unsupported_pattern(),
            TokenKind::Punctuation(Punctuation::LeftParen) => self.parse_pattern_parens(mark),
            _ => self.unexpected_pattern(),
        };

        if matches!(
            self.ast().get(tree).kind,
            TreeKind::Ident(_) | TreeKind::This(_) | TreeKind::Super(_) | TreeKind::Select(_)
        ) {
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
                let Some((name, backquoted)) = self.current_selector_name() else {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedToken,
                        "expected a selector after `.` in pattern",
                    );
                    return tree;
                };
                self.advance();
                tree = self.alloc_from(
                    mark,
                    TreeKind::Select(Select {
                        qualifier: tree,
                        name,
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

    fn parse_given_pattern(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        let typed_mark = self.mark();
        let wildcard = *TermName::new(self.names.intern("_")).as_name();
        let wildcard_ident = self.alloc_from(
            typed_mark,
            TreeKind::Ident(Ident {
                name: wildcard,
                backquoted: false,
            }),
        );
        let tpt = self.parse_refined_type();
        let typed = self.alloc_from(
            typed_mark,
            TreeKind::Typed(TypedExpr {
                expr: wildcard_ident,
                tpt,
            }),
        );

        // Dotty encodes `given T` as a wildcard Bind around a typed wildcard,
        // with a `Given` modifier on the Bind. The shared node keeps that
        // source-level distinction in its `given` bit.
        self.alloc_from(
            mark,
            TreeKind::Bind(Bind {
                name: wildcard,
                body: typed,
                given: true,
            }),
        )
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
        // A colon following a numeric literal is lexed as `ColonOp` when the
        // literal is prefixed with `-`.  It belongs to Pattern1's typed
        // literal production, not to the infix-pattern layer.  Keep symbolic
        // operators such as `::` available for right-associative patterns.
        if matches!(spelling, "|" | "@" | ":") || self.current_is_structural_operator() {
            return None;
        }
        self.pattern_operand_offset()?;
        Some(*self.intern_current_term_name().ok()?.as_name())
    }

    fn current_starts_symbolic_pattern_extractor(&mut self) -> bool {
        symbolic_pattern_extractor_at(self, 0)
    }

    fn pattern_operand_offset(&mut self) -> Option<usize> {
        let offset = match self.cursor.lookahead(1).kind {
            TokenKind::Newline | TokenKind::Newlines => 2,
            _ => 1,
        };
        can_start_simple_pattern_at(self, offset).then_some(offset)
    }

    /// Finds a pattern-alternative bar after optional physical line separators.
    /// Comments are trivia, so any lines they occupy arrive as newline tokens.
    /// Leave those separators untouched unless a bar follows, preserving the
    /// case arrow and the next case clause as their own grammar boundaries.
    fn pattern_alternative_operator_offset(&mut self) -> Option<usize> {
        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        (self.token_text_at(offset) == Some("|")).then_some(offset)
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

    fn token_text_at(&mut self, offset: usize) -> Option<&'src str> {
        let token = self.cursor.lookahead(offset);
        self.source.slice(token.span).ok()
    }

    fn unexpected_pattern(&mut self) -> TreeId<Untyped> {
        let position = self.current_span();
        self.report(ParseDiagnosticKind::ExpectedPattern, "expected a pattern");
        if self.current().kind != TokenKind::Eof
            && !self.current_is_structural_operator()
            && !matches!(
                self.current().kind,
                TokenKind::Punctuation(
                    Punctuation::Comma
                        | Punctuation::RightParen
                        | Punctuation::RightBrace
                        | Punctuation::Semicolon
                )
            )
        {
            self.advance();
        }
        self.error_pattern(position)
    }

    fn unsupported_pattern(&mut self) -> TreeId<Untyped> {
        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "this pattern form is not supported yet",
        );
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

fn is_pattern_variable<S: TokenSource>(parser: &Parser<'_, '_, S>, tree: TreeId<Untyped>) -> bool {
    let TreeKind::Ident(identifier) = &parser.ast().get(tree).kind else {
        return false;
    };
    let name = parser.names.resolve(identifier.name.text());
    if matches!(name, "false" | "true" | "null") {
        return false;
    }
    name.chars()
        .next()
        .is_some_and(|first| first == '_' || (first.is_alphabetic() && first.is_lowercase()))
}

fn is_ident_named<S: TokenSource>(
    parser: &Parser<'_, '_, S>,
    tree: TreeId<Untyped>,
    spelling: &str,
) -> bool {
    matches!(
        &parser.ast().get(tree).kind,
        TreeKind::Ident(identifier) if parser.names.resolve(identifier.name.text()) == spelling
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
                | TokenKind::CharLiteral
                | TokenKind::InterpolationId
                | TokenKind::Keyword(HardKeyword::True)
                | TokenKind::Keyword(HardKeyword::False)
                | TokenKind::Keyword(HardKeyword::Null)
                | TokenKind::Keyword(HardKeyword::This)
                | TokenKind::Keyword(HardKeyword::Super)
                | TokenKind::Quote
                | TokenKind::Punctuation(Punctuation::LeftParen)
        )
}

fn pattern_alternative_operand_offset<S: TokenSource>(
    parser: &mut Parser<'_, '_, S>,
) -> Option<usize> {
    let mut offset = 0;
    while matches!(
        parser.cursor.lookahead(offset).kind,
        TokenKind::Newline | TokenKind::Newlines
    ) {
        offset += 1;
    }
    can_start_simple_pattern_at(parser, offset).then_some(offset)
}

fn can_start_simple_pattern_at<S: TokenSource>(
    parser: &mut Parser<'_, '_, S>,
    offset: usize,
) -> bool {
    can_start_simple_pattern_kind(parser.cursor.lookahead(offset).kind)
        || symbolic_pattern_extractor_at(parser, offset)
        || (parser.cursor.lookahead(offset).kind == TokenKind::Operator
            && parser
                .source
                .slice(parser.cursor.lookahead(offset).span)
                .ok()
                == Some("-")
            && is_numeric_literal(parser.cursor.lookahead(offset + 1).kind))
}

fn symbolic_pattern_extractor_at<S: TokenSource>(
    parser: &mut Parser<'_, '_, S>,
    offset: usize,
) -> bool {
    let token = parser.cursor.lookahead(offset);
    if !matches!(token.kind, TokenKind::Operator | TokenKind::ColonOp) {
        return false;
    }
    let Some(spelling) = parser.source.slice(token.span).ok() else {
        return false;
    };
    if matches!(
        spelling,
        "=" | "=>" | "<-" | "<:" | ">:" | "<%" | "@" | "?=>" | "#" | "=>>" | "|" | ":"
    ) {
        return false;
    }
    parser.cursor.lookahead(offset + 1).kind == TokenKind::Punctuation(Punctuation::LeftParen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{
        Alternative, Annotated, Apply, Bind, Ident, SplicePattern, This, Tuple, UntypedNode,
    };
    use dotty_core::{NameInterner, Punctuation, TextRange};

    fn assert_unchecked_type(
        tree: &dotty_core::ast::AstArena<Untyped>,
        tpt: TreeId<Untyped>,
        names: &NameInterner,
    ) {
        let TreeKind::Annotated(Annotated { expr, annotation }) = tree.get(tpt).kind else {
            panic!("expected the type pattern's type to be Annotated");
        };
        let TreeKind::Apply(application) = &tree.get(annotation).kind else {
            panic!("expected @unchecked to be an annotation application");
        };
        let TreeKind::Select(constructor) = tree.get(application.function).kind else {
            panic!("expected the annotation constructor selection");
        };
        let TreeKind::New(new) = tree.get(constructor.qualifier).kind else {
            panic!("expected a New node for the annotation");
        };
        let TreeKind::Ident(annotation_type) = tree.get(new.tpt).kind else {
            panic!("expected the annotation type name");
        };
        assert_eq!(names.resolve(annotation_type.name.text()), "unchecked");
        assert!(matches!(
            tree.get(expr).kind,
            TreeKind::Ident(_) | TreeKind::Select(_)
        ));
    }

    #[test]
    fn parses_unchecked_annotation_on_a_bound_variable_type_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x: T @unchecked",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::ColonOp, 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Identifier, 6, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Typed(typed) = result.ast.get(result.root).kind else {
            panic!("expected a typed pattern");
        };
        assert!(matches!(
            result.ast.get(typed.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_unchecked_type(&result.ast, typed.tpt, &names);
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 15).unwrap()
        );
    }

    #[test]
    fn parses_an_interpolated_string_pattern_with_a_simple_splice() {
        let source = "s\"hello $name!\"";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 9),
                token(TokenKind::Identifier, 9, 13),
                token(TokenKind::StringPart, 13, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let pattern = parser.pattern();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            &parser.ast().get(pattern).kind
        else {
            panic!("expected an interpolated-string pattern");
        };

        assert_eq!(parser.names.resolve(interpolation.prefix.text()), "s");
        assert_eq!(interpolation.parts.len(), 3);
        assert!(matches!(
            parser.ast().get(interpolation.parts[1]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(pattern).position.unwrap().span().range(),
            TextRange::new(0, 15).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_symbolic_colon_operator_extractor_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "::(head, tail)",
            vec![
                token(TokenKind::ColonOp, 0, 2),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 2, 3),
                token(TokenKind::Identifier, 3, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Apply(application) = &result.ast.get(result.root).kind else {
            panic!("expected a source-level extractor application");
        };
        let TreeKind::Ident(function) = &result.ast.get(application.function).kind else {
            panic!("expected the symbolic extractor name");
        };
        assert_eq!(names.resolve(function.name.text()), "::");
        assert_eq!(application.args.len(), 2);
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 14).unwrap()
        );
    }

    #[test]
    fn parses_a_symbolic_extractor_nested_in_a_named_extractor_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Left(::(e, es))",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
                token(TokenKind::ColonOp, 5, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Apply(outer) = &result.ast.get(result.root).kind else {
            panic!("expected the outer extractor application");
        };
        let TreeKind::Apply(inner) = &result.ast.get(outer.args[0]).kind else {
            panic!("expected the nested symbolic extractor application");
        };
        let TreeKind::Ident(function) = &result.ast.get(inner.function).kind else {
            panic!("expected the symbolic extractor name");
        };
        assert_eq!(names.resolve(function.name.text()), "::");
    }

    #[test]
    fn parses_an_interpolated_string_pattern_with_a_braced_splice() {
        let source = "s\"hello ${name}\"";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 9, 10),
                token(TokenKind::Identifier, 10, 14),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 14, 15),
                token(TokenKind::StringPart, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let pattern = parser.pattern();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            &parser.ast().get(pattern).kind
        else {
            panic!("expected an interpolated-string pattern");
        };

        assert_eq!(interpolation.parts.len(), 3);
        assert!(matches!(
            parser.ast().get(interpolation.parts[1]).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(
            parser.ast().get(pattern).position.unwrap().span().range(),
            TextRange::new(0, 16).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn treats_a_wildcard_in_an_interpolated_pattern_splice_as_a_pattern() {
        let source = "s\"${_}\"";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 5, 6),
                token(TokenKind::StringPart, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let pattern = parser.pattern();
        parser.report_escaping_placeholders();

        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            &parser.ast().get(pattern).kind
        else {
            panic!("expected an interpolated-string pattern");
        };
        let TreeKind::Block(block) = &parser.ast().get(interpolation.parts[1]).kind else {
            panic!("expected a braced pattern splice");
        };
        let TreeKind::Ident(wildcard) = &parser.ast().get(block.expr).kind else {
            panic!("expected wildcard pattern identifier");
        };
        assert_eq!(parser.names.resolve(wildcard.name.text()), "_");
        assert!(block.stats.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_unchecked_annotation_on_a_wildcard_type_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "_: T @unchecked",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::ColonOp, 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Identifier, 6, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Typed(typed) = result.ast.get(result.root).kind else {
            panic!("expected a typed wildcard pattern");
        };
        let TreeKind::Ident(wildcard) = result.ast.get(typed.expr).kind else {
            panic!("expected the wildcard identifier");
        };
        assert_eq!(names.resolve(wildcard.name.text()), "_");
        assert_unchecked_type(&result.ast, typed.tpt, &names);
    }

    #[test]
    fn parses_unchecked_annotation_on_a_qualified_type_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x: pkg.T @unchecked",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::ColonOp, 1, 2),
                token(TokenKind::Identifier, 3, 6),
                token(TokenKind::Punctuation(Punctuation::Dot), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Identifier, 10, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Typed(typed) = result.ast.get(result.root).kind else {
            panic!("expected a typed pattern");
        };
        let TreeKind::Annotated(annotated) = result.ast.get(typed.tpt).kind else {
            panic!("expected an annotated type");
        };
        let TreeKind::Select(selection) = result.ast.get(annotated.expr).kind else {
            panic!("expected the qualified type reference");
        };
        assert_eq!(names.resolve(selection.name.text()), "T");
        assert_unchecked_type(&result.ast, typed.tpt, &names);
    }

    #[test]
    fn recovers_from_a_missing_unchecked_annotation_type_at_pattern_end() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x: T @",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::ColonOp, 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Typed(_)
        ));
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
        assert_eq!(
            result.diagnostics[0]
                .legacy_message()
                .expect("legacy parser diagnostic"),
            "expected an annotation type after `@`"
        );
    }

    #[test]
    fn quoted_pattern_can_start_an_alternative_after_a_newline() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "|\n'{ $x }",
            vec![
                token(TokenKind::Operator, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Quote, 2, 3),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );
        parser.advance();

        assert_eq!(pattern_alternative_operand_offset(&mut parser), Some(1));
    }

    #[test]
    fn quoted_pattern_parses_simple_dollar_identifiers_as_splice_patterns() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "'{ $x + $y }",
            vec![
                token(TokenKind::Quote, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 1, 2),
                token(TokenKind::Identifier, 3, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty());
        let TreeKind::Quote(quote) = &result.ast.get(result.root).kind else {
            panic!("Dotty's parser-level quoted pattern is represented by Quote");
        };
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = &result.ast.get(quote.body).kind
        else {
            panic!("expected the quoted infix pattern body");
        };
        assert!(matches!(
            result.ast.get(infix.left).kind,
            TreeKind::SplicePattern(SplicePattern { .. })
        ));
        assert!(matches!(
            result.ast.get(infix.right).kind,
            TreeKind::SplicePattern(SplicePattern { .. })
        ));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 12).unwrap()
        );
    }

    #[test]
    fn quoted_pattern_braced_splice_parses_a_pattern_body() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "'{ ${x | y} }",
            vec![
                token(TokenKind::Quote, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty());
        let TreeKind::Quote(quote) = &result.ast.get(result.root).kind else {
            panic!("expected a parser-level Quote node");
        };
        let TreeKind::SplicePattern(splice) = &result.ast.get(quote.body).kind else {
            panic!("expected a pattern splice");
        };
        assert!(matches!(
            result.ast.get(splice.body).kind,
            TreeKind::Alternative(_)
        ));
        assert_eq!(
            result.ast.get(quote.body).position.unwrap().span().range(),
            TextRange::new(3, 11).unwrap()
        );
    }

    #[test]
    fn quoted_pattern_splice_keeps_type_and_term_suffixes_as_source_nodes() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "'{ $f[Int](x, y) }",
            vec![
                token(TokenKind::Quote, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 1, 2),
                token(TokenKind::Identifier, 3, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 5, 6),
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 9, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::Comma), 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty());
        let TreeKind::Quote(quote) = &result.ast.get(result.root).kind else {
            panic!("expected a parser-level Quote node");
        };
        let TreeKind::Apply(application) = &result.ast.get(quote.body).kind else {
            panic!("the splice's application suffix remains an Apply node");
        };
        let TreeKind::TypeApply(type_application) = &result.ast.get(application.function).kind
        else {
            panic!("the splice's type-application suffix remains a TypeApply node");
        };
        assert!(matches!(
            result.ast.get(type_application.function).kind,
            TreeKind::SplicePattern(SplicePattern { .. })
        ));
        assert_eq!(application.args.len(), 2);
        assert_eq!(
            result
                .ast
                .get(application.function)
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(3, 10).unwrap()
        );
        assert_eq!(
            result.ast.get(quote.body).position.unwrap().span().range(),
            TextRange::new(3, 16).unwrap()
        );
    }

    #[test]
    fn unterminated_quoted_pattern_splice_reports_errors_and_returns_a_tree() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "'{ ${x",
            vec![
                token(TokenKind::Quote, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 1, 2),
                token(TokenKind::Identifier, 3, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(!result.diagnostics.is_empty());
        let TreeKind::Quote(quote) = &result.ast.get(result.root).kind else {
            panic!("malformed quote-pattern input still returns its partial Quote");
        };
        assert!(matches!(
            result.ast.get(quote.body).kind,
            TreeKind::SplicePattern(SplicePattern { .. })
        ));
    }

    #[test]
    fn parses_an_identifier_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Ident(Ident { .. })
        ));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn parses_a_symbolic_name_after_a_pattern_selection_dot() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x.##",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Select(selection) = result.ast.get(result.root).kind else {
            panic!("expected a symbolic pattern selection");
        };
        assert_eq!(names.resolve(selection.name.text()), "##");
        assert!(!selection.backquoted);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn leaves_reserved_hash_unconsumed_after_a_pattern_selection_dot() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x.#",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let _ = parser.pattern();

        assert_eq!(parser.current().kind, TokenKind::Operator);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0]
                .legacy_message()
                .expect("legacy parser diagnostic"),
            "expected a selector after `.` in pattern"
        );
    }

    #[test]
    fn leaves_reserved_context_function_arrow_unconsumed_after_a_pattern_selection_dot() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x.=>>",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
                token(TokenKind::Operator, 2, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let _ = parser.pattern();

        assert_eq!(parser.current().kind, TokenKind::Operator);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0]
                .legacy_message()
                .expect("legacy parser diagnostic"),
            "expected a selector after `.` in pattern"
        );
    }

    #[test]
    fn parses_a_wildcard_pattern_as_the_source_name() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "_",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Ident(identifier) = result.ast.get(result.root).kind else {
            panic!("expected wildcard identifier");
        };
        assert_eq!(names.resolve(identifier.name.text()), "_");
    }

    #[test]
    fn parses_parenthesized_and_tuple_patterns_recursively() {
        let mut names = NameInterner::new();
        let parser = parser_for(
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
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { ref elements }))
                if elements.len() == 2
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_character_literal_pattern_through_the_shared_literal_decoder() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "'B'",
            vec![
                token(TokenKind::CharLiteral, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Char(66)
            })
        ));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 3).unwrap()
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn keeps_extractor_patterns_as_source_level_apply_trees() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo(x)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Apply(Apply { ref args, .. }) if args.len() == 1
        ));
        assert!(!matches!(
            result.ast.get(result.root).kind,
            TreeKind::UnApply(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn rejects_a_direct_wildcard_in_a_pattern_type_application() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo[?]",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::TypeApply(ref type_apply) = result.ast.get(result.root).kind else {
            panic!("expected a pattern type application");
        };
        assert!(matches!(
            result.ast.get(type_apply.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
    }

    #[test]
    fn rejects_a_bounded_wildcard_in_a_pattern_type_application_and_preserves_the_closer() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo[? >: L <: U]",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::Operator, 6, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::TypeApply(ref type_apply) = result.ast.get(result.root).kind else {
            panic!("expected a pattern type application");
        };
        assert!(matches!(
            result.ast.get(type_apply.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::ExpectedType
        );
    }

    #[test]
    fn allows_a_wildcard_inside_a_nested_pattern_type_application() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo[List[?]]",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 3, 4),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 8, 9),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::TypeApply(ref type_apply) = result.ast.get(result.root).kind else {
            panic!("expected the outer pattern type application");
        };
        let TreeKind::AppliedTypeTree(ref inner) = result.ast.get(type_apply.args[0]).kind else {
            panic!("expected a nested applied type");
        };
        assert!(matches!(
            result.ast.get(inner.args[0]).kind,
            TreeKind::TypeBoundsTree(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_binder_and_preserves_its_body() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x @ Foo(y)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Bind(Bind { name, body, given }) = result.ast.get(result.root).kind else {
            panic!("expected bind pattern");
        };
        assert!(!given);
        assert_eq!(names.resolve(name.text()), "x");
        assert!(matches!(result.ast.get(body).kind, TreeKind::Apply(_)));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_alternatives_above_infix_patterns() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "a | b",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Alternative(Alternative { ref alternatives })
                if alternatives.len() == 2
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn continues_an_alternative_pattern_after_a_newline_following_pipe() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Left(x) |\nRight(x)",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Operator, 8, 9),
                token(TokenKind::Newline, 9, 10),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 15, 16),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightParen), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Alternative(Alternative { ref alternatives }) if alternatives.len() == 2
        ));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 18).unwrap()
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn continues_a_pattern_alternative_after_comment_lines_before_the_pipe() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "A\n//c\n| B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Newline, 5, 6),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Alternative(Alternative { ref alternatives }) if alternatives.len() == 2
        ));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 9).unwrap()
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn recovers_from_a_line_leading_alternative_without_a_rhs() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "A\n|",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Alternative(Alternative { ref alternatives }) if alternatives.len() == 2
        ));
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::ExpectedPattern
        );
    }

    #[test]
    fn accepts_super_as_an_alternative_pattern_start() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x | super.foo",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Keyword(HardKeyword::Super), 4, 9),
                token(TokenKind::Punctuation(Punctuation::Dot), 9, 10),
                token(TokenKind::Identifier, 10, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Alternative(Alternative { ref alternatives })
                if alternatives.len() == 2
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn accepts_a_negative_literal_after_an_infix_pattern_operator() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x :: -1",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::ColonOp, 2, 4),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::IntegerLiteral, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn reports_mixed_associativity_in_pattern_infix_operators() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "a + b +: c",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        assert!(
            result.diagnostics[0]
                .legacy_message()
                .expect("legacy parser diagnostic")
                .contains("mixed")
        );
    }

    #[test]
    fn reports_mixed_associativity_when_right_associative_operator_comes_first() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "a +: b + c",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        assert!(
            result.diagnostics[0]
                .legacy_message()
                .expect("legacy parser diagnostic")
                .contains("mixed")
        );
    }

    #[test]
    fn accepts_a_newline_after_an_infix_pattern_operator() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "head ::\ntail",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::ColonOp, 5, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn accepts_an_interpolated_string_pattern_after_a_newline_operator() {
        let source = "head ::\ns\"$name\"";
        let mut names = NameInterner::new();
        let parser = parser_for(
            source,
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::ColonOp, 5, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::InterpolationId, 8, 9),
                token(TokenKind::StringPart, 9, 11),
                token(TokenKind::Identifier, 11, 15),
                token(TokenKind::StringPart, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) = result.ast.get(result.root).kind
        else {
            panic!("expected an infix pattern");
        };
        assert!(matches!(
            result.ast.get(infix.right).kind,
            TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(_))
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_negative_numeric_typed_pattern_as_typed() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "-42: Int",
            vec![
                token(TokenKind::Operator, 0, 1),
                token(TokenKind::IntegerLiteral, 1, 3),
                token(TokenKind::ColonOp, 3, 4),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Typed(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_super_member_pattern_without_symbol_resolution() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "super.member",
            vec![
                token(TokenKind::Keyword(HardKeyword::Super), 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Select(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_this_member_pattern_as_a_selection() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "this.member",
            vec![
                token(TokenKind::Keyword(HardKeyword::This), 0, 4),
                token(TokenKind::Punctuation(Punctuation::Dot), 4, 5),
                token(TokenKind::Identifier, 5, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Select(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_qualified_this_member_pattern_with_source_spans() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Outer.this.member",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Punctuation(Punctuation::Dot), 5, 6),
                token(TokenKind::Keyword(HardKeyword::This), 6, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Select(selection) = result.ast.get(result.root).kind else {
            panic!("expected selection over qualified this");
        };
        let TreeKind::This(This {
            qual: Some(qualifier),
        }) = result.ast.get(selection.qualifier).kind
        else {
            panic!("expected qualified this receiver");
        };

        assert!(qualifier.is_type());
        assert_eq!(
            result
                .ast
                .get(selection.qualifier)
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(0, 10).unwrap()
        );
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 17).unwrap()
        );
        assert!(result.diagnostics.is_empty());
        let member = selection.name;
        drop(result);
        assert_eq!(names.resolve(qualifier.text()), "Outer");
        assert_eq!(names.resolve(member.text()), "member");
    }

    #[test]
    fn reports_a_missing_pattern_after_an_alternative() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x |",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Alternative(_)
        ));
    }

    #[test]
    fn reports_a_missing_pattern_after_a_binder() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x @",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Bind(_)
        ));
    }

    #[test]
    fn recovers_from_an_unclosed_extractor_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo(",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Apply(_)
        ));
    }

    #[test]
    fn preserves_full_spans_for_bind_and_alternative_patterns() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x @ Foo(y)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();
        let bind = result.ast.get(result.root);

        assert_eq!(
            bind.position.unwrap().span().range(),
            TextRange::new(0, 10).unwrap()
        );
        let TreeKind::Bind(Bind { body, .. }) = bind.kind else {
            panic!("expected bind pattern");
        };
        assert_eq!(
            result.ast.get(body).position.unwrap().span().range(),
            TextRange::new(4, 10).unwrap()
        );
    }

    #[test]
    fn parses_given_patterns_as_marked_typed_wildcard_binders() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "given Context",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(result.diagnostics.is_empty());
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 13).unwrap()
        );
        let TreeKind::Bind(Bind { name, body, given }) = result.ast.get(result.root).kind else {
            panic!("Dotty represents a given pattern as a marked wildcard Bind");
        };
        assert!(given);
        assert_eq!(names.resolve(name.text()), "_");

        let typed = result.ast.get(body);
        assert_eq!(
            typed.position.unwrap().span().range(),
            TextRange::new(6, 13).unwrap()
        );
        let TreeKind::Typed(typed) = typed.kind else {
            panic!("given pattern body should be typed");
        };
        let TreeKind::Ident(wildcard) = result.ast.get(typed.expr).kind else {
            panic!("typed pattern should contain a wildcard");
        };
        assert_eq!(names.resolve(wildcard.name.text()), "_");
        assert_eq!(
            result.ast.get(typed.expr).position.unwrap().span().range(),
            TextRange::new(6, 6).unwrap()
        );
    }

    #[test]
    fn missing_given_pattern_type_recovers_with_a_diagnostic() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "given",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Bind(Bind { given: true, .. })
        ));
    }

    #[test]
    fn reports_one_focused_diagnostic_for_a_top_level_sequence_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x*",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
    }

    #[test]
    fn reports_one_focused_diagnostic_for_a_bound_sequence_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x @ _*",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
    }

    #[test]
    fn parses_an_extractor_sequence_pattern_as_typed_wildcard_star() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo(xs*)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 6),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Apply(application) = &result.ast.get(result.root).kind else {
            panic!("expected extractor application");
        };
        let TreeKind::Typed(typed) = &result.ast.get(application.args[0]).kind else {
            panic!("expected a typed sequence pattern");
        };
        let TreeKind::Ident(pattern_variable) = &result.ast.get(typed.expr).kind else {
            panic!("expected a pattern variable");
        };
        let TreeKind::Ident(wildcard_star) = &result.ast.get(typed.tpt).kind else {
            panic!("expected the `_*` sequence marker");
        };
        assert_eq!(names.resolve(pattern_variable.name.text()), "xs");
        assert_eq!(names.resolve(wildcard_star.name.text()), "_*");
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 8).unwrap()
        );
        assert_eq!(
            result
                .ast
                .get(application.args[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(4, 7).unwrap()
        );
        assert_eq!(
            result.ast.get(typed.tpt).position.unwrap().span().range(),
            TextRange::new(6, 7).unwrap()
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_bound_wildcard_sequence_with_dotty_typed_shape() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo(x @ _*)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Apply(application) = &result.ast.get(result.root).kind else {
            panic!("expected extractor application");
        };
        let arg = application.args[0];
        let TreeKind::Typed(typed) = &result.ast.get(arg).kind else {
            panic!("expected Dotty's typed bound-sequence shape");
        };
        let TreeKind::Ident(binder) = &result.ast.get(typed.expr).kind else {
            panic!("expected the binder identifier as the typed expression");
        };
        let TreeKind::Ident(sequence_marker) = &result.ast.get(typed.tpt).kind else {
            panic!("expected the `_*` sequence marker as the type");
        };

        assert_eq!(names.resolve(binder.name.text()), "x");
        assert_eq!(names.resolve(sequence_marker.name.text()), "_*");
        assert_eq!(
            result.ast.get(arg).position.unwrap().span().range(),
            TextRange::new(4, 10).unwrap()
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_trailing_extractor_sequence_pattern_after_a_comma() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo(head, tail*)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Identifier, 10, 14),
                token(TokenKind::Operator, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        let TreeKind::Apply(application) = &result.ast.get(result.root).kind else {
            panic!("expected extractor application");
        };
        assert_eq!(application.args.len(), 2);
        assert!(matches!(
            result.ast.get(application.args[1]).kind,
            TreeKind::Typed(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn rejects_a_sequence_marker_inside_a_tuple_pattern() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "(x*)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightParen), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 4).unwrap()
        );
    }

    #[test]
    fn keeps_ordinary_star_infix_pattern_parsing_unchanged() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "a * b",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn rejects_a_sequence_marker_after_a_stable_pattern_name() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "Foo(Items*)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );
        let result = parser.parse_pattern_fragment();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 11).unwrap()
        );
    }
}
