//! Shared parsing infrastructure for class, trait, and object template bodies.
//!
//! A template body is a source-ordered list of members.  Unlike an expression
//! block, it must retain every entry, including expressions, because later
//! definition parsing and semantic phases decide how those entries are
//! interpreted.  The actual class-like definition parser is layered on top of
//! this helper.

use dotty_core::ast::{Modifier, Modifiers, ValDef};
use dotty_core::{HardKeyword, Punctuation, TermName, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseDiagnosticKind, Parser, RecoverySet};

/// Delimiters accepted by the template-body parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateBody {
    Braced,
    Indented,
}

pub(crate) struct TemplateBodyResult {
    pub self_val: Option<TreeId<Untyped>>,
    pub members: Vec<TreeId<Untyped>>,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a template body and preserves all statements in source order.
    ///
    /// The opening and closing delimiter belong to this helper.  In the
    /// indented form the scanner has already classified the layout and the
    /// parser only consumes the resulting `Indent`/`Outdent` tokens.
    pub(crate) fn parse_template_body(&mut self, body: TemplateBody) -> TemplateBodyResult {
        self.parse_template_body_with_feedback(body, None)
    }

    pub(crate) fn parse_template_body_with_feedback(
        &mut self,
        body: TemplateBody,
        feedback_indent: Option<u32>,
    ) -> TemplateBodyResult {
        let (opening, closing) = match body {
            TemplateBody::Braced => (
                TokenKind::Punctuation(Punctuation::LeftBrace),
                TokenKind::Punctuation(Punctuation::RightBrace),
            ),
            TemplateBody::Indented => (TokenKind::Indent, TokenKind::Outdent),
        };

        if !self.expect(opening) {
            return TemplateBodyResult {
                self_val: None,
                members: Vec::new(),
            };
        }

        let body_indent = self.source_line_indent_prefix(self.current().span.start());
        let result = self.with_placeholder_scope(|parser| {
            parser.with_location(Location::InBlock, |parser| {
                parser.with_block_end(Some(closing), |parser| {
                    parser.parse_template_members(closing, body_indent)
                })
            })
        });

        if body == TemplateBody::Indented {
            if let Some(indent_offset) = feedback_indent {
                self.observe_outdented_region(indent_offset);
            } else if self.current().kind != TokenKind::Outdent {
                self.observe_outdented();
            }
        }
        if !self.accept(closing) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                format!("expected {closing:?} to close template body"),
            );
        }

        result
    }

    fn parse_template_members(
        &mut self,
        closing: TokenKind,
        body_indent: String,
    ) -> TemplateBodyResult {
        let mut members = Vec::new();
        self.consume_template_separators(closing);
        let self_val = self.parse_template_self();
        self.consume_template_separators(closing);

        loop {
            if self.current().kind == TokenKind::EndMarker {
                if !self.end_marker_matches_next(members.last().copied()) {
                    break;
                }
                if !self.consume_end_marker(members.last().copied()) {
                    break;
                }
                self.consume_template_separators(closing);
                continue;
            }
            if !members.is_empty()
                && closing == TokenKind::Outdent
                && !self.last_advance_was_outdent
            {
                self.feedback_template_outdent(&body_indent);
            }
            if self.template_body_ended(closing) {
                break;
            }

            let checkpoint = self.cursor.checkpoint();
            self.last_advance_was_outdent = false;
            let allow_secondary_constructor = self.context.secondary_constructor_allowed
                && (self.current().kind == TokenKind::Keyword(HardKeyword::Def)
                    || self.starts_definition_prefix());
            let statement = self
                .with_secondary_constructor_allowed(allow_secondary_constructor, |parser| {
                    parser.parse_statement(Location::InBlock)
                });
            let ended_nested_indented_body = self.last_advance_was_outdent;
            match statement {
                crate::statements::ParsedStatement::Definition(tree)
                | crate::statements::ParsedStatement::Expression(tree) => members.push(tree),
                crate::statements::ParsedStatement::Many(trees) => members.extend(trees),
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a template body",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if closing == TokenKind::Outdent {
                // A nested method/body parser may already have consumed its
                // own Outdent. Re-check before consuming the following
                // newline, or the template's closing Outdent can be delayed
                // until after the next enclosing statement.
                self.feedback_template_outdent(&body_indent);
            }
            if self.is_template_separator(self.current().kind) {
                self.consume_template_separators(closing);
            } else if ended_nested_indented_body && !self.template_body_ended(closing) {
                // The nested body's Outdent is also the boundary before the
                // next statement/member, including inside a braced template
                // where the outer body itself has no layout delimiter.
            } else if !self.template_body_ended(closing) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "expected a template member separator",
                );
                self.recover_until(RecoverySet::Statement);
                self.consume_template_separators(closing);
            }

            while self.current().kind == TokenKind::EndMarker {
                if !self.end_marker_matches_next(members.last().copied()) {
                    break;
                }
                if !self.consume_end_marker(members.last().copied()) {
                    return TemplateBodyResult { self_val, members };
                }
                self.consume_template_separators(closing);
            }
        }

        TemplateBodyResult { self_val, members }
    }

    fn parse_template_self(&mut self) -> Option<TreeId<Untyped>> {
        if !self.starts_template_self() {
            return None;
        }

        let mark = self.mark();
        let is_this = self.current().kind == TokenKind::Keyword(HardKeyword::This);
        let name = if is_this {
            self.advance();
            TermName::new(self.names.intern("_"))
        } else {
            match self.intern_current_term_name() {
                Ok(name) => {
                    self.advance();
                    name
                }
                Err(_) => return None,
            }
        };
        let has_colon = self.accept_self_colon();
        if is_this && !has_colon {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `:` after `this` in a template self type",
            );
        }
        let tpt = if has_colon {
            self.with_parse_kind(crate::ParseKind::Type, |parser| parser.parse_infix_type())
        } else {
            self.synthetic_type_tree_at(mark.start())
        };
        if self.starts_unsupported_self_type_tail() {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "compound template self types are not supported yet",
            );
            self.recover_self_type_tail();
        }
        if self.current_is_arrow() {
            self.observe_self_arrow();
            self.advance();
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` after a template self type",
            );
        }

        Some(self.alloc_from(
            mark,
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs: None,
                metadata: Modifiers {
                    modifiers: vec![Modifier::PrivateLocal],
                    ..Modifiers::default()
                },
            }),
        ))
    }

    fn starts_unsupported_self_type_tail(&mut self) -> bool {
        match self.current().kind {
            TokenKind::Operator | TokenKind::ColonOp => !self.current_text_is("=>"),
            TokenKind::Keyword(HardKeyword::With) => true,
            TokenKind::Punctuation(Punctuation::LeftBracket) => true,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                matches!(
                    self.cursor.lookahead(1).kind,
                    TokenKind::Identifier | TokenKind::BackquotedIdentifier
                )
            }
            _ => false,
        }
    }

    fn recover_self_type_tail(&mut self) {
        while !self.current_is_arrow()
            && !matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
                    | TokenKind::Eof
                    | TokenKind::Punctuation(Punctuation::RightBrace)
            )
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn starts_template_self(&mut self) -> bool {
        let is_name = matches!(
            self.current().kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::Keyword(HardKeyword::This)
        );
        if !is_name {
            return false;
        }
        if self.cursor.lookahead(1).kind == TokenKind::Operator
            && self.source.slice(self.cursor.lookahead(1).span).ok() == Some("=>")
        {
            return true;
        }
        if !is_self_colon(self.cursor.lookahead(1).kind) {
            return false;
        }
        let mut offset = 2;
        loop {
            let token = self.cursor.lookahead(offset);
            if token.kind == TokenKind::Punctuation(Punctuation::LeftBrace) {
                let previous = self.cursor.lookahead(offset.saturating_sub(1));
                if previous.kind == TokenKind::Operator
                    && self.source.slice(previous.span).ok() == Some("^")
                {
                    let mut depth = 0usize;
                    loop {
                        match self.cursor.lookahead(offset).kind {
                            TokenKind::Punctuation(Punctuation::LeftBrace) => depth += 1,
                            TokenKind::Punctuation(Punctuation::RightBrace) => {
                                depth -= 1;
                                if depth == 0 {
                                    offset = offset.saturating_add(1);
                                    break;
                                }
                            }
                            TokenKind::Eof => return false,
                            _ => {}
                        }
                        offset = offset.saturating_add(1);
                    }
                    continue;
                }
                return false;
            }
            if matches!(
                token.kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
                    | TokenKind::Eof
                    | TokenKind::Punctuation(Punctuation::RightBrace)
            ) {
                return false;
            }
            if token.kind == TokenKind::Operator && self.source.slice(token.span).ok() == Some("=>")
            {
                return true;
            }
            offset = offset.saturating_add(1);
        }
    }

    fn accept_self_colon(&mut self) -> bool {
        if is_self_colon(self.current().kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn feedback_template_outdent(&mut self, body_indent: &str) {
        let indent_offset = if matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            self.cursor.lookahead(1).span.start()
        } else {
            self.current().span.start()
        };
        if self.current().kind != TokenKind::Eof
            && self.current().kind != TokenKind::Outdent
            && self.current().kind != TokenKind::Punctuation(dotty_core::Punctuation::RightBrace)
            && self
                .source_line_indent_prefix(indent_offset)
                .starts_with(body_indent)
        {
            return;
        }
        if self.current().kind != TokenKind::Outdent && self.current().kind != TokenKind::Eof {
            // The scanner owns the active layout stack and decides whether
            // this position closes a nested indented expression.
            self.observe_outdented();
        }
    }

    fn template_body_ended(&self, closing: TokenKind) -> bool {
        self.current().kind == closing || self.current().kind == TokenKind::Eof
    }

    fn is_template_separator(&self, kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Punctuation(Punctuation::Semicolon)
        )
    }

    fn consume_template_separators(&mut self, closing: TokenKind) {
        while self.is_template_separator(self.current().kind) && !self.template_body_ended(closing)
        {
            self.advance();
        }
    }

    fn source_line_indent_prefix(&self, offset: u32) -> String {
        let source = self.source.as_str();
        let end = (offset as usize).min(source.len());
        let line_start = source[..end]
            .rfind(['\n', '\r', '\u{000c}', '\u{001a}'])
            .map_or(0, |index| index + 1);
        source[line_start..end]
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t'))
            .collect()
    }
}

const fn is_self_colon(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Punctuation(Punctuation::Colon)
            | TokenKind::ColonFollow
            | TokenKind::ColonOp
            | TokenKind::ColonEol
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::UntypedNode;
    use dotty_core::{
        HardKeyword, NameInterner, ScannerEvent, SourceId, SourceText, TextRange, Token,
        TokenSource, TreeKind,
    };

    struct YieldTemplateTokenSource {
        tokens: Vec<Token>,
        index: usize,
        tuple_line_outdents: usize,
        yield_body_outdent: bool,
    }

    impl TokenSource for YieldTemplateTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index.min(self.tokens.len() - 1)]
        }

        fn position(&self) -> usize {
            self.index
        }

        fn advance(&mut self) {
            self.index = (self.index + 1).min(self.tokens.len() - 1);
        }

        fn lookahead(&mut self, offset: usize) -> &Token {
            &self.tokens[(self.index + offset).min(self.tokens.len() - 1)]
        }

        fn observe(&mut self, event: ScannerEvent) {
            match event {
                ScannerEvent::Indented | ScannerEvent::IndentedFrom { .. }
                    if self.current().kind != TokenKind::Indent
                        && self.lookahead(1).kind != TokenKind::Indent =>
                {
                    let offset = self.current().span.end();
                    self.tokens.insert(
                        self.index + 1,
                        Token::new(TokenKind::Indent, TextRange::new(offset, offset).unwrap()),
                    );
                }
                ScannerEvent::Outdented | ScannerEvent::OutdentedRegion { .. }
                    if self.current().kind == TokenKind::Newline =>
                {
                    // The first feedback closes the indented method RHS; the
                    // second closes its enclosing anonymous template. The
                    // outer yield block is at the same indentation as the
                    // tuple and must remain open for that final expression.
                    if self.tuple_line_outdents < 2 {
                        let offset = self.current().span.start();
                        self.tokens.insert(
                            self.index,
                            Token::new(TokenKind::Outdent, TextRange::new(offset, offset).unwrap()),
                        );
                        self.tuple_line_outdents += 1;
                    }
                }
                ScannerEvent::Outdented | ScannerEvent::OutdentedRegion { .. }
                    if self.current().kind == TokenKind::Punctuation(Punctuation::RightBrace)
                        && !self.yield_body_outdent =>
                {
                    let offset = self.current().span.start();
                    self.tokens.insert(
                        self.index,
                        Token::new(TokenKind::Outdent, TextRange::new(offset, offset).unwrap()),
                    );
                    self.yield_body_outdent = true;
                }
                _ => {}
            }
        }
    }

    fn source_token(
        source: &str,
        kind: TokenKind,
        text: &str,
        occurrence: usize,
    ) -> dotty_core::Token {
        let (start, value) = source
            .match_indices(text)
            .nth(occurrence)
            .expect("test token text exists in source");
        token(kind, start as u32, (start + value.len()) as u32)
    }

    fn nested_match_template_tokens(
        source: &str,
        ending: Vec<dotty_core::Token>,
    ) -> Vec<dotty_core::Token> {
        let mut tokens = vec![
            source_token(
                source,
                TokenKind::Punctuation(Punctuation::LeftBrace),
                "{",
                0,
            ),
            source_token(source, TokenKind::Keyword(HardKeyword::Def), "def", 0),
            source_token(source, TokenKind::Identifier, "f", 0),
            source_token(source, TokenKind::Operator, "=", 0),
            source_token(source, TokenKind::Identifier, "value", 0),
            source_token(source, TokenKind::Keyword(HardKeyword::Match), "match", 0),
            source_token(source, TokenKind::Newline, "\n", 0),
            token(TokenKind::Indent, 0, 0),
            source_token(source, TokenKind::Keyword(HardKeyword::Case), "case", 0),
            source_token(source, TokenKind::Identifier, "A", 0),
            source_token(source, TokenKind::Operator, "=>", 0),
            source_token(source, TokenKind::Identifier, "result", 0),
            token(TokenKind::Outdent, 0, 0),
        ];
        tokens.extend(ending);
        tokens.push(source_token(
            source,
            TokenKind::Punctuation(Punctuation::RightBrace),
            "}",
            0,
        ));
        tokens.push(token(
            TokenKind::Eof,
            source.len() as u32,
            source.len() as u32,
        ));
        tokens
    }

    fn yield_template_tokens(source: &str) -> Vec<Token> {
        let source_token = |kind, text: &str, occurrence| {
            let (start, found) = source
                .match_indices(text)
                .nth(occurrence)
                .expect("test token text exists in source");
            token(kind, start as u32, (start + found.len()) as u32)
        };
        let mut tokens = vec![
            source_token(TokenKind::Punctuation(Punctuation::LeftBrace), "{", 0),
            source_token(TokenKind::Keyword(HardKeyword::For), "for", 0),
            source_token(TokenKind::Identifier, "x", 0),
            source_token(TokenKind::Operator, "<-", 0),
            source_token(TokenKind::Identifier, "values", 0),
            source_token(TokenKind::Keyword(HardKeyword::Yield), "yield", 0),
            source_token(TokenKind::Keyword(HardKeyword::Val), "val", 0),
            source_token(TokenKind::Identifier, "cleanup", 0),
            source_token(TokenKind::Operator, "=", 0),
            source_token(TokenKind::Keyword(HardKeyword::New), "new", 0),
            source_token(TokenKind::Identifier, "C", 0),
            source_token(TokenKind::ColonEol, ":", 0),
            source_token(TokenKind::Keyword(HardKeyword::Def), "def", 0),
            source_token(TokenKind::Identifier, "transform", 0),
            source_token(TokenKind::Operator, "=", 1),
            source_token(TokenKind::Identifier, "item", 0),
            token(TokenKind::Newline, 0, 0),
            source_token(TokenKind::Punctuation(Punctuation::LeftParen), "(", 0),
            source_token(TokenKind::Identifier, "x", 1),
            source_token(TokenKind::Punctuation(Punctuation::Comma), ",", 0),
            source_token(TokenKind::Identifier, "y", 0),
            source_token(TokenKind::Punctuation(Punctuation::RightParen), ")", 0),
            source_token(TokenKind::Punctuation(Punctuation::RightBrace), "}", 0),
        ];
        let tuple_start = source.find("(x, y)").unwrap() as u32;
        let newline_index = 16;
        tokens[newline_index] = token(
            TokenKind::Newline,
            source.find("item").unwrap() as u32 + 4,
            tuple_start,
        );
        tokens.push(token(
            TokenKind::Eof,
            source.len() as u32,
            source.len() as u32,
        ));
        tokens
    }

    #[test]
    fn braced_template_body_preserves_definitions_and_expressions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ val x = 1\nfoo }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::Val), 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 9),
                token(TokenKind::IntegerLiteral, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Braced).members;

        assert_eq!(members.len(), 2);
        assert!(matches!(
            parser.ast().get(members[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(
            parser.ast().get(members[1]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn same_line_template_members_still_require_a_separator() {
        let source = "{ def first = 1 private def second = 2 }";
        let tokens = vec![
            source_token(
                source,
                TokenKind::Punctuation(Punctuation::LeftBrace),
                "{",
                0,
            ),
            source_token(source, TokenKind::Keyword(HardKeyword::Def), "def", 0),
            source_token(source, TokenKind::Identifier, "first", 0),
            source_token(source, TokenKind::Operator, "=", 0),
            source_token(source, TokenKind::IntegerLiteral, "1", 0),
            source_token(
                source,
                TokenKind::Keyword(HardKeyword::Private),
                "private",
                0,
            ),
            source_token(source, TokenKind::Keyword(HardKeyword::Def), "def", 1),
            source_token(source, TokenKind::Identifier, "second", 0),
            source_token(source, TokenKind::Operator, "=", 1),
            source_token(source, TokenKind::IntegerLiteral, "2", 0),
            source_token(
                source,
                TokenKind::Punctuation(Punctuation::RightBrace),
                "}",
                0,
            ),
            token(TokenKind::Eof, source.len() as u32, source.len() as u32),
        ];
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, tokens, &mut names);

        parser.parse_template_body(TemplateBody::Braced);

        assert!(
            parser.diagnostics().iter().any(|diagnostic| {
                diagnostic.message() == "expected a template member separator"
            })
        );
    }

    #[test]
    fn indented_template_body_consumes_layout_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "  first\n  second",
            vec![
                token(TokenKind::Indent, 0, 0),
                token(TokenKind::Identifier, 2, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Outdent, 16, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Indented).members;

        assert_eq!(members.len(), 2);
        assert!(
            members
                .iter()
                .all(|member| matches!(parser.ast().get(*member).kind, TreeKind::Ident(_)))
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn nested_method_outdent_closes_new_template_before_yield_tuple() {
        let source = "{\n  for x <- values yield\n    val cleanup = new C:\n      def transform =\n        item\n    (x, y)\n}";
        let tokens = YieldTemplateTokenSource {
            tokens: yield_template_tokens(source),
            index: 0,
            tuple_line_outdents: 0,
            yield_body_outdent: false,
        };
        let mut names = NameInterner::new();
        let parser = Parser::new(
            SourceText::new(source).unwrap(),
            SourceId::from_index(1),
            tokens,
            &mut names,
        );

        let result = parser.parse_expression_fragment();

        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let TreeKind::Block(outer) = &result.ast.get(result.root).kind else {
            panic!("expected outer block");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ForYield(for_yield)) =
            &result.ast.get(outer.expr).kind
        else {
            panic!("expected a for/yield expression");
        };
        let TreeKind::Block(body) = &result.ast.get(for_yield.body).kind else {
            panic!("expected a multi-statement yield body");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            result.ast.get(body.expr).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
    }

    #[test]
    fn nested_match_outdent_separates_the_following_template_method() {
        let source = "{ def f = value match\n  case A => result\n  def g = next }";
        let mut names = NameInterner::new();
        let next_method = vec![
            source_token(source, TokenKind::Keyword(HardKeyword::Def), "def", 1),
            source_token(source, TokenKind::Identifier, "g", 0),
            source_token(source, TokenKind::Operator, "=", 1),
            source_token(source, TokenKind::Identifier, "next", 0),
        ];
        let mut parser = parser_for(
            source,
            nested_match_template_tokens(source, next_method),
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Braced).members;

        assert_eq!(members.len(), 2);
        assert!(matches!(
            parser.ast().get(members[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(matches!(
            parser.ast().get(members[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser.diagnostics().is_empty(),
            "unexpected diagnostics: {:?}",
            parser.diagnostics()
        );
    }

    #[test]
    fn nested_match_outdent_preserves_the_following_end_marker() {
        let source = "{ def f = value match\n  case A => result\n  end f }";
        let mut names = NameInterner::new();
        let end_marker = vec![
            source_token(source, TokenKind::EndMarker, "end", 0),
            source_token(source, TokenKind::Identifier, "f", 2),
        ];
        let mut parser = parser_for(
            source,
            nested_match_template_tokens(source, end_marker),
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Braced).members;

        assert_eq!(members.len(), 1);
        assert!(matches!(
            parser.ast().get(members[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        let method_span = parser
            .ast()
            .get(members[0])
            .position
            .unwrap()
            .span()
            .range();
        assert_eq!(method_span.end(), source.find("end f").unwrap() as u32 + 5);
        assert!(
            parser.diagnostics().is_empty(),
            "unexpected diagnostics: {:?}",
            parser.diagnostics()
        );
    }

    #[test]
    fn compares_indentation_prefixes_instead_of_their_widths() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "\tinner\n value",
            vec![token(TokenKind::Identifier, 1, 6)],
            &mut names,
        );

        assert_eq!(parser.source_line_indent_prefix(1), "\t");
        assert_eq!(parser.source_line_indent_prefix(8), " ");
        assert!(
            !parser
                .source_line_indent_prefix(8)
                .starts_with(parser.source_line_indent_prefix(1).as_str())
        );
    }

    #[test]
    fn empty_template_body_has_no_synthetic_expression_member() {
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

        let members = parser.parse_template_body(TemplateBody::Braced).members;

        assert!(members.is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_template_self_arrow_without_creating_a_nested_layout_region() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ self => value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let result = parser.parse_template_body(TemplateBody::Braced);

        assert!(matches!(
            parser.ast().get(result.self_val.expect("self value")).kind,
            TreeKind::ValDef(_)
        ));
        assert_eq!(result.members.len(), 1);
        assert!(matches!(
            parser.ast().get(result.members[0]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_compound_self_types_without_losing_the_body_boundary() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ self: A & B => value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::Punctuation(Punctuation::Colon), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 22),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let result = parser.parse_template_body(TemplateBody::Braced);

        assert!(result.self_val.is_some());
        assert_eq!(result.members.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_self_type_application_at_the_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ self: Parent[T] => value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::Punctuation(Punctuation::Colon), 6, 7),
                token(TokenKind::Identifier, 8, 14),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 14, 15),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 16, 17),
                token(TokenKind::Operator, 18, 20),
                token(TokenKind::Identifier, 21, 26),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let result = parser.parse_template_body(TemplateBody::Braced);

        assert!(result.self_val.is_some());
        assert_eq!(result.members.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_empty_capture_set_in_a_template_self_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ this: ofBoolean^{} => value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::This), 2, 6),
                token(TokenKind::Punctuation(Punctuation::Colon), 6, 7),
                token(TokenKind::Identifier, 8, 17),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 19, 20),
                token(TokenKind::Operator, 21, 23),
                token(TokenKind::Identifier, 24, 29),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            capture_checking: true,
            ..crate::ParserFeatures::default()
        });

        let result = parser.parse_template_body(TemplateBody::Braced);

        assert!(
            result.self_val.is_some(),
            "current={:?}, diagnostics={:?}",
            parser.current().kind,
            parser.diagnostics()
        );
        assert_eq!(result.members.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        let self_value = result.self_val.expect("self type should be retained");
        let TreeKind::ValDef(self_value) = &parser.ast().get(self_value).kind else {
            panic!("expected the template self value");
        };
        assert!(matches!(
            parser.ast().get(self_value.tpt).kind,
            TreeKind::Annotated(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_typed_this_self_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ this: Parent => value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::This), 2, 6),
                token(TokenKind::Punctuation(Punctuation::Colon), 6, 7),
                token(TokenKind::Identifier, 8, 14),
                token(TokenKind::Operator, 15, 17),
                token(TokenKind::Identifier, 18, 23),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let result = parser.parse_template_body(TemplateBody::Braced);

        let self_val = result.self_val.expect("self value");
        let TreeKind::ValDef(self_def) = &parser.ast().get(self_val).kind else {
            panic!("expected self ValDef");
        };
        assert!(matches!(
            parser.ast().get(self_def.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert_eq!(result.members.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_untyped_this_self_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ this => value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::This), 2, 6),
                token(TokenKind::Operator, 7, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let result = parser.parse_template_body(TemplateBody::Braced);

        assert!(result.self_val.is_some());
        assert_eq!(result.members.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
    }
}
