/// Parser-facing token kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A recoverable lexical error.
    Error,
    /// An identifier.
    Identifier,
    /// An identifier enclosed in backquotes.
    BackquotedIdentifier,
    /// A Scala quote marker before a quoted term or type.
    Quote,
    /// A legacy quoted identifier such as `'name`.
    QuoteId,
    /// The start of a Scala XML literal.
    XmlStart,
    /// A symbolic operator.
    Operator,
    /// A Scala hard keyword.
    Keyword(HardKeyword),
    /// A delimiter or punctuation token.
    Punctuation(Punctuation),
    /// A colon used as an operator.
    ColonOp,
    /// A colon following a type or term prefix.
    ColonFollow,
    /// A colon confirmed by parser feedback to end a line.
    ColonEol,
    /// A `case class` pair fused by scanner post-processing.
    CaseClass,
    /// A `case object` pair fused by scanner post-processing.
    CaseObject,
    /// An `end` token recognized as an end marker.
    EndMarker,
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
    /// The Scala `if` keyword token.
    If,
    /// The Scala `for` keyword token.
    For,
    /// The Scala `else` keyword token.
    Else,
    /// The Scala `this` keyword token.
    This,
    /// The Scala `null` keyword token.
    Null,
    /// The Scala `new` keyword token.
    New,
    /// The Scala `super` keyword token.
    Super,
    /// The Scala `abstract` keyword token.
    Abstract,
    /// The Scala `final` keyword token.
    Final,
    /// The Scala `private` keyword token.
    Private,
    /// The Scala `protected` keyword token.
    Protected,
    /// The Scala `override` keyword token.
    Override,
    /// The Scala `extends` keyword token.
    Extends,
    /// The Scala `true` keyword token.
    True,
    /// The Scala `false` keyword token.
    False,
    /// The Scala `class` keyword token.
    Class,
    /// The Scala `import` keyword token.
    Import,
    /// The Scala `package` keyword token.
    Package,
    /// The Scala `do` keyword token.
    Do,
    /// The Scala `sealed` keyword token.
    Sealed,
    /// The Scala `throw` keyword token.
    Throw,
    /// The Scala `try` keyword token.
    Try,
    /// The Scala `catch` keyword token.
    Catch,
    /// The Scala `finally` keyword token.
    Finally,
    /// The Scala `while` keyword token.
    While,
    /// The Scala `return` keyword token.
    Return,
    /// The Scala `with` keyword token.
    With,
    /// The Scala `case` keyword token.
    Case,
    /// The Scala `val` keyword token.
    Val,
    /// The Scala `implicit` keyword token.
    Implicit,
    /// The Scala `var` keyword token.
    Var,
    /// The Scala `def` keyword token.
    Def,
    /// The Scala `type` keyword token.
    Type,
    /// The Scala `object` keyword token.
    Object,
    /// The Scala `yield` keyword token.
    Yield,
    /// The Scala `trait` keyword token.
    Trait,
    /// The Scala `match` keyword token.
    Match,
    /// The Scala `lazy` keyword token.
    Lazy,
    /// The Scala `then` keyword token.
    Then,
    /// The Scala `forSome` keyword token.
    ForSome,
    /// The Scala `enum` keyword token.
    Enum,
    /// The Scala `given` keyword token.
    Given,
    /// The Scala `export` keyword token.
    Export,
    /// The Scala `macro` keyword token.
    Macro,
    /// The Scala `end` keyword token.
    End,
}

/// Punctuation that is structurally distinct from a raw operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punctuation {
    /// The `,` punctuation token.
    Comma,
    /// The `;` punctuation token.
    Semicolon,
    /// The `.` punctuation token.
    Dot,
    /// The `:` punctuation token.
    Colon,
    /// The `(` punctuation token.
    LeftParen,
    /// The `)` punctuation token.
    RightParen,
    /// The `[` punctuation token.
    LeftBracket,
    /// The `]` punctuation token.
    RightBracket,
    /// The `{` punctuation token.
    LeftBrace,
    /// The `}` punctuation token.
    RightBrace,
}
