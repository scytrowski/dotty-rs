use dotty_core::{
    AstArena, NameInterner, SourceId, SourceSpan, SourceText, SourceTextError, TermName, Token,
    TokenSource, Tree, TreeId, TreeKind, TypeName, Untyped,
};

use crate::Cursor;

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
        Self {
            cursor: Cursor::new(tokens),
            source,
            source_id,
            names,
            ast: AstArena::new(),
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

    /// Returns the parser's untyped AST arena.
    pub fn ast(&self) -> &AstArena<Untyped> {
        &self.ast
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{TextRange, TokenKind, TokenValue};

    struct SingleTokenSource {
        token: Token,
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
}
