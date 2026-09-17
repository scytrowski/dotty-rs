use dotty_source::TextRange;

/// Source material that is not a parser-facing token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trivia {
    pub kind: TriviaKind,
    pub span: TextRange,
}

/// Kinds of trivia preserved by the raw lexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriviaKind {
    Spaces,
    Tabs,
    OtherWhitespace,
    Newline,
    LineComment,
    BlockComment,
}
