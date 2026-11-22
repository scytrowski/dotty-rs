use dotty_core::source::TextRange;

/// Source material that is not a parser-facing token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trivia {
    /// Whitespace or comment category of this retained source slice.
    pub kind: TriviaKind,
    /// Half-open UTF-8 byte range in the originating source buffer.
    pub span: TextRange,
}

/// Kinds of trivia preserved by the raw lexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriviaKind {
    /// One or more space characters.
    Spaces,
    /// One or more horizontal tab characters.
    Tabs,
    /// Whitespace other than spaces, tabs, or line breaks.
    OtherWhitespace,
    /// A source line break.
    Newline,
    /// Comment extending to the next line break or end of input.
    LineComment,
    /// Block comment, retaining its source extent.
    BlockComment,
}
