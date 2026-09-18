use dotty_core::{
    AstArena, NameInterner, SourceId, SourceSpan, SourceText, SourceTextError, Span, TermName,
    TextRange, Token, TokenKind, TokenSource, Tree, TreeId, TreeKind, TypeName, Untyped,
};

use crate::Cursor;
use crate::Mark;

/// Stateful input and allocation context for the handwritten parser.
pub struct Parser<'src, 'names, S>
where
    S: TokenSource,
{
    pub(crate) cursor: Cursor<S>,
    pub(crate) source: SourceText<'src>,
    pub(crate) source_id: SourceId,
    pub(crate) names: &'names mut NameInterner,
    pub(crate) ast: AstArena<Untyped>,
    pub(crate) last_real_token_end: u32,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: TokenSource,
{
    /// Creates a parser state over a source-backed token stream.
    pub fn new(
        source: SourceText<'src>,
        source_id: SourceId,
        tokens: S,
        names: &'names mut NameInterner,
    ) -> Self {
        let cursor = Cursor::new(tokens);
        let last_real_token_end = if is_zero_width_synthetic(cursor.kind()) {
            cursor.current().span.start()
        } else {
            0
        };

        Self {
            cursor,
            source,
            source_id,
            names,
            ast: AstArena::new(),
            last_real_token_end,
        }
    }

    /// Returns the source file identifier associated with this parser.
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// Returns the current parser-facing token.
    pub fn current(&self) -> &Token {
        self.cursor.current()
    }

    /// Advances the parser and records the end of a real token.
    pub fn advance(&mut self) {
        let token = self.current();
        if !is_zero_width_synthetic(token.kind) && token.kind != TokenKind::Eof {
            self.last_real_token_end = token.span.end();
        }
        self.cursor.advance();
    }

    /// Creates a mark at the current parser position.
    pub fn mark(&self) -> Mark {
        let start = if is_zero_width_synthetic(self.current().kind) {
            self.last_real_token_end
        } else {
            self.current().span.start()
        };
        Mark { start }
    }

    /// Builds a source span from `mark` to the end of the last real token.
    pub fn span_from(&self, mark: Mark) -> SourceSpan {
        let end = self.last_real_token_end.max(mark.start);
        let range = TextRange::new(mark.start, end).expect("span endpoints are ordered");
        SourceSpan::new(self.source_id, Span::without_point(range))
    }

    /// Returns the source spelling of `token` without allocating.
    pub fn token_text(&self, token: &Token) -> Result<&'src str, SourceTextError> {
        self.source.slice(token.span)
    }

    /// Returns the source spelling of the current token without allocating.
    pub fn current_text(&self) -> Result<&'src str, SourceTextError> {
        let span = self.current().span;
        self.source.slice(span)
    }

    /// Interns the current token spelling in the term namespace.
    pub fn intern_current_term_name(&mut self) -> Result<TermName, SourceTextError> {
        let span = self.current().span;
        let text = self.source.slice(span)?;
        Ok(TermName::new(self.names.intern(text)))
    }

    /// Interns the current token spelling in the type namespace.
    pub fn intern_current_type_name(&mut self) -> Result<TypeName, SourceTextError> {
        let span = self.current().span;
        let text = self.source.slice(span)?;
        Ok(TypeName::new(self.names.intern(text)))
    }

    /// Allocates an untyped AST node with an optional source position.
    pub fn alloc(
        &mut self,
        kind: TreeKind<Untyped>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        self.ast.alloc(Tree {
            kind,
            position,
            ty: (),
        })
    }

    /// Allocates an untyped AST node spanning from `mark` to the last real token.
    pub fn alloc_from(&mut self, mark: Mark, kind: TreeKind<Untyped>) -> TreeId<Untyped> {
        let position = self.span_from(mark);
        self.alloc(kind, Some(position))
    }

    /// Returns the parser's untyped AST arena.
    pub fn ast(&self) -> &AstArena<Untyped> {
        &self.ast
    }
}

const fn is_zero_width_synthetic(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Indent | TokenKind::Outdent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{TextRange, TokenKind, TokenValue};

    struct SingleTokenSource {
        token: Token,
    }

    struct SequenceTokenSource {
        tokens: Vec<Token>,
        index: usize,
    }

    impl TokenSource for SingleTokenSource {
        fn current(&self) -> &Token {
            &self.token
        }

        fn advance(&mut self) {}

        fn lookahead(&mut self, _n: usize) -> &Token {
            &self.token
        }

        fn observe(&mut self, _event: dotty_core::ScannerEvent) {}
    }

    impl TokenSource for SequenceTokenSource {
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
        text: &'src str,
        span: TextRange,
        names: &'names mut NameInterner,
    ) -> Parser<'src, 'names, SingleTokenSource> {
        let source = SourceText::new(text).expect("valid source");
        let token = Token {
            kind: TokenKind::Identifier,
            span,
            value: TokenValue::None,
        };
        Parser::new(
            source,
            SourceId::from_index(1),
            SingleTokenSource { token },
            names,
        )
    }

    fn parser_with_tokens<'src, 'names>(
        text: &'src str,
        tokens: Vec<Token>,
        names: &'names mut NameInterner,
    ) -> Parser<'src, 'names, SequenceTokenSource> {
        Parser::new(
            SourceText::new(text).expect("valid source"),
            SourceId::from_index(1),
            SequenceTokenSource { tokens, index: 0 },
            names,
        )
    }

    #[test]
    fn current_text_uses_the_source_backed_token_span() {
        let mut names = NameInterner::new();
        let parser = parser_for("żółw", TextRange::new(0, 2).unwrap(), &mut names);

        assert_eq!(parser.current_text(), Ok("ż"));
    }

    #[test]
    fn token_text_rejects_a_range_outside_the_source() {
        let mut names = NameInterner::new();
        let parser = parser_for("abc", TextRange::new(1, 4).unwrap(), &mut names);

        assert_eq!(
            parser.token_text(parser.current()),
            Err(SourceTextError::RangeOutOfBounds {
                range: TextRange::new(1, 4).unwrap(),
                byte_len: 3,
            })
        );
    }

    #[test]
    fn current_term_name_is_interned_from_source_text() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("value", TextRange::new(0, 5).unwrap(), &mut names);
        let name = parser.intern_current_term_name().expect("valid token span");
        drop(parser);

        assert_eq!(names.resolve(name.as_name().text()), "value");
    }

    #[test]
    fn current_type_name_uses_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("Value", TextRange::new(0, 5).unwrap(), &mut names);
        let name = parser.intern_current_type_name().expect("valid token span");
        drop(parser);

        assert!(name.as_name().is_type());
        assert_eq!(names.resolve(name.as_name().text()), "Value");
    }

    #[test]
    fn allocation_preserves_kind_and_source_position() {
        let mut names = NameInterner::new();
        let mut parser = parser_for("value", TextRange::new(0, 5).unwrap(), &mut names);
        let span = SourceSpan::new(
            parser.source_id(),
            dotty_core::Span::without_point(TextRange::new(0, 1).unwrap()),
        );
        let name = *parser
            .intern_current_term_name()
            .expect("valid token span")
            .as_name();
        let id = parser.alloc(TreeKind::Ident(dotty_core::ast::Ident { name }), Some(span));

        assert!(matches!(parser.ast().get(id).kind, TreeKind::Ident(_)));
        assert_eq!(parser.ast().get(id).position, Some(span));
    }

    #[test]
    fn span_from_covers_one_real_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let mark = parser.mark();

        parser.advance();

        assert_eq!(
            parser.span_from(mark).span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn span_from_covers_parenthesized_input_through_the_last_real_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "(x)",
            vec![
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    0,
                    1,
                ),
                token(TokenKind::Identifier, 1, 2),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    2,
                    3,
                ),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );
        let mark = parser.mark();

        parser.advance();
        parser.advance();
        parser.advance();

        assert_eq!(
            parser.span_from(mark).span().range(),
            TextRange::new(0, 3).unwrap()
        );
    }

    #[test]
    fn synthetic_outdent_does_not_extend_the_last_real_token_end() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Outdent, 1, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        parser.advance();
        let mark = parser.mark();

        assert_eq!(mark.start(), 1);
        assert_eq!(
            parser.span_from(mark).span().range(),
            TextRange::new(1, 1).unwrap()
        );
    }

    #[test]
    fn eof_mark_is_a_zero_width_span_after_the_last_real_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        parser.advance();

        assert_eq!(
            parser.span_from(parser.mark()).span().range(),
            TextRange::new(1, 1).unwrap()
        );
    }

    #[test]
    fn alloc_from_assigns_the_span_built_from_its_mark() {
        let mut names = NameInterner::new();
        let mut parser = parser_with_tokens(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let mark = parser.mark();
        let name = *parser
            .intern_current_term_name()
            .expect("valid token span")
            .as_name();

        parser.advance();
        let id = parser.alloc_from(mark, TreeKind::Ident(dotty_core::ast::Ident { name }));

        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }
}
