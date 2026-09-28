/// Parser-to-scanner feedback events used by Scala 3 layout handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannerEvent {
    ColonEol {
        in_template: bool,
    },
    Indented,
    Outdented,
    /// Closes a parser-requested indentation region using a grammar delimiter
    /// without exposing an `Outdent` token to the parser.
    OutdentedByDelimiter,
    ArrowIndented,
    SelfArrow,
}
