use dotty_core::source::TextRange;

pub use dotty_core::token::{HardKeyword, Punctuation};

/// A source item emitted by the raw lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawItem {
    /// A lexical token, including lexical errors and end of input.
    Token(RawToken),
    /// Whitespace or a comment retained by the lossless raw stream.
    Trivia(crate::Trivia),
}

/// A lexical token before contextual scanner processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawToken {
    /// Raw lexical category; interpretation of spelling remains separate.
    pub kind: RawTokenKind,
    /// Half-open UTF-8 byte range in the originating source buffer.
    pub span: TextRange,
}

/// Raw lexical categories needed by the first lexer increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTokenKind {
    /// A lexical error token; inspect the separately collected diagnostics.
    Error,
    /// End-of-input marker.
    Eof,
    /// Unquoted identifier spelling.
    Identifier,
    /// Identifier written between backquotes.
    BackquotedIdentifier,
    /// Quote introducer before expression or type quote syntax.
    Quote,
    /// Quote-prefixed identifier spelling.
    QuoteId,
    /// Start of XML syntax; later handling determines support.
    XmlStart,
    /// Operator identifier spelling.
    Operator,
    /// Hard keyword recognized by the raw lexer.
    Keyword(HardKeyword),
    /// Delimiter or separator recognized by the raw lexer.
    Punctuation(Punctuation),
    /// Character literal spelling.
    CharLiteral,
    /// Unsuffixed whole-number literal, retaining its radix.
    IntegerLiteral,
    /// Unsuffixed decimal literal spelling.
    DecimalLiteral,
    /// Unsuffixed exponent-form literal spelling.
    ExponentLiteral,
    /// Integer literal with a Long suffix.
    LongLiteral,
    /// Numeric literal with a Float suffix.
    FloatLiteral,
    /// Numeric literal with a Double suffix.
    DoubleLiteral,
    /// String literal spelling, including its delimiters.
    StringLiteral,
    /// Interpolator identifier immediately preceding a string.
    InterpolationId,
    /// String segment emitted while scanning an interpolated string.
    StringPart,
}
