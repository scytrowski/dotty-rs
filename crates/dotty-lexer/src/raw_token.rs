use dotty_source::TextRange;

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
