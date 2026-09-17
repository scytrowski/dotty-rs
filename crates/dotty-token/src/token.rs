use dotty_source::TextRange;

use crate::{TokenKind, TokenValue};

/// A parser-facing token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: TextRange,
    pub value: TokenValue,
}

impl Token {
    /// Creates a token without an associated value.
    pub const fn new(kind: TokenKind, span: TextRange) -> Self {
        Self {
            kind,
            span,
            value: TokenValue::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_token_with_an_empty_value() {
        let span = TextRange::new(1, 3).expect("valid range");
        let token = Token::new(TokenKind::Eof, span);

        assert_eq!(token.kind, TokenKind::Eof);
        assert_eq!(token.span, span);
        assert_eq!(token.value, TokenValue::None);
    }
}
