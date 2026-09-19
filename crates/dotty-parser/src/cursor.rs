use dotty_core::{ScannerEvent, Token, TokenKind, TokenSource};

/// Small adapter that keeps parser token access in one place.
pub struct Cursor<S> {
    source: S,
}

impl<S> Cursor<S>
where
    S: TokenSource,
{
    /// Creates a cursor over a parser-facing token source.
    pub const fn new(source: S) -> Self {
        Self { source }
    }

    /// Returns the current parser-facing token.
    pub fn current(&self) -> &Token {
        self.source.current()
    }

    /// Returns the current token kind.
    pub fn kind(&self) -> TokenKind {
        self.current().kind
    }

    /// Returns whether the current token has `kind`.
    pub fn at(&self, kind: TokenKind) -> bool {
        self.kind() == kind
    }

    /// Advances the token source.
    pub fn advance(&mut self) {
        self.source.advance();
    }

    /// Returns the token `n` positions after the current token.
    pub fn lookahead(&mut self, n: usize) -> &Token {
        self.source.lookahead(n)
    }

    /// Forwards parser context to the scanner.
    pub fn observe(&mut self, event: ScannerEvent) {
        self.source.observe(event);
    }

    /// Consumes the current token when it has `kind`.
    pub fn accept(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Punctuation, TextRange, TokenValue};

    struct VecTokenSource {
        tokens: Vec<Token>,
        index: usize,
        observed: Vec<ScannerEvent>,
    }

    impl VecTokenSource {
        fn new(tokens: Vec<Token>) -> Self {
            assert!(!tokens.is_empty(), "test token sources need an EOF token");
            Self {
                tokens,
                index: 0,
                observed: Vec::new(),
            }
        }
    }

    impl TokenSource for VecTokenSource {
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

        fn observe(&mut self, event: ScannerEvent) {
            self.observed.push(event);
        }
    }

    fn token(kind: TokenKind, start: u32, end: u32) -> Token {
        Token {
            kind,
            span: TextRange::new(start, end).expect("valid test range"),
            value: TokenValue::None,
        }
    }

    fn tokens() -> Vec<Token> {
        vec![
            token(TokenKind::Identifier, 0, 1),
            token(TokenKind::Punctuation(Punctuation::Dot), 1, 2),
            token(TokenKind::Eof, 2, 2),
        ]
    }

    #[test]
    fn current_and_kind_read_the_current_token() {
        let cursor = Cursor::new(VecTokenSource::new(tokens()));

        assert_eq!(cursor.kind(), TokenKind::Identifier);
        assert_eq!(cursor.current().span, TextRange::new(0, 1).unwrap());
    }

    #[test]
    fn advance_moves_to_the_next_token() {
        let mut cursor = Cursor::new(VecTokenSource::new(tokens()));

        cursor.advance();

        assert!(cursor.at(TokenKind::Punctuation(Punctuation::Dot)));
    }

    #[test]
    fn lookahead_reads_without_advancing() {
        let mut cursor = Cursor::new(VecTokenSource::new(tokens()));

        assert_eq!(
            cursor.lookahead(1).kind,
            TokenKind::Punctuation(Punctuation::Dot)
        );
        assert!(cursor.at(TokenKind::Identifier));
    }

    #[test]
    fn lookahead_beyond_eof_stays_at_eof() {
        let mut cursor = Cursor::new(VecTokenSource::new(tokens()));

        assert_eq!(cursor.lookahead(99).kind, TokenKind::Eof);
    }

    #[test]
    fn accept_consumes_only_the_expected_kind() {
        let mut cursor = Cursor::new(VecTokenSource::new(tokens()));

        assert!(!cursor.accept(TokenKind::Punctuation(Punctuation::Dot)));
        assert!(cursor.at(TokenKind::Identifier));
        assert!(cursor.accept(TokenKind::Identifier));
        assert!(cursor.at(TokenKind::Punctuation(Punctuation::Dot)));
    }

    #[test]
    fn observe_forwards_scanner_feedback() {
        let mut cursor = Cursor::new(VecTokenSource::new(tokens()));

        cursor.observe(ScannerEvent::Indented);

        assert_eq!(cursor.source.observed, vec![ScannerEvent::Indented]);
    }

    #[test]
    fn advance_at_eof_does_not_move_past_the_eof_token() {
        let mut cursor = Cursor::new(VecTokenSource::new(tokens()));

        cursor.advance();
        cursor.advance();

        assert!(cursor.at(TokenKind::Eof));
    }
}
