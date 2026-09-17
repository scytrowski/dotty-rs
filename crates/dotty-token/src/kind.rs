/// Parser-facing token kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// End of the parser-facing token stream.
    Eof,
}
