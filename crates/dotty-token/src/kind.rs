/// Parser-facing token kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A recoverable lexical error.
    Error,
    /// An identifier.
    Identifier,
    /// An identifier enclosed in backquotes.
    BackquotedIdentifier,
    /// A symbolic operator.
    Operator,
    /// A Scala hard keyword.
    Keyword(HardKeyword),
    /// A delimiter or punctuation token.
    Punctuation(Punctuation),
    /// A character literal.
    CharLiteral,
    /// An integer literal.
    IntegerLiteral,
    /// A decimal literal.
    DecimalLiteral,
    /// An exponent literal.
    ExponentLiteral,
    /// A long literal.
    LongLiteral,
    /// A float literal.
    FloatLiteral,
    /// A double literal.
    DoubleLiteral,
    /// A non-interpolated string literal.
    StringLiteral,
    /// The identifier preceding an interpolated string.
    InterpolationId,
    /// A literal part of an interpolated string.
    StringPart,
    /// A physical line break that separates statements.
    Newline,
    /// A statement separator following a blank line.
    Newlines,
    /// A synthetic indentation opener.
    Indent,
    /// A synthetic indentation closer.
    Outdent,
    /// End of the parser-facing token stream.
    Eof,
}

/// Alphabetic keywords recognized by the Scala 3.9.0 scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardKeyword {
    If,
    For,
    Else,
    This,
    Null,
    New,
    Super,
    Abstract,
    Final,
    Private,
    Protected,
    Override,
    Extends,
    True,
    False,
    Class,
    Import,
    Package,
    Do,
    Sealed,
    Throw,
    Try,
    Catch,
    Finally,
    While,
    Return,
    With,
    Case,
    Val,
    Implicit,
    Var,
    Def,
    Type,
    Object,
    Yield,
    Trait,
    Match,
    Lazy,
    Then,
    ForSome,
    Enum,
    Given,
    Export,
    Macro,
    End,
}

/// Punctuation that is structurally distinct from a raw operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punctuation {
    Comma,
    Semicolon,
    Dot,
    Colon,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
}
