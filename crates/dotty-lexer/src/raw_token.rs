use dotty_source::TextRange;

pub use dotty_token::{HardKeyword, Punctuation};

/// A source item emitted by the raw lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawItem {
    Token(RawToken),
    Trivia(crate::Trivia),
}

/// A lexical token before contextual scanner processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawToken {
    pub kind: RawTokenKind,
    pub span: TextRange,
}

/// Raw lexical categories needed by the first lexer increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTokenKind {
    Error,
    Eof,
    Identifier,
    BackquotedIdentifier,
    Operator,
    Keyword(HardKeyword),
    Punctuation(Punctuation),
    CharLiteral,
    IntegerLiteral,
    DecimalLiteral,
    ExponentLiteral,
    LongLiteral,
    FloatLiteral,
    DoubleLiteral,
    StringLiteral,
    InterpolationId,
    StringPart,
}
