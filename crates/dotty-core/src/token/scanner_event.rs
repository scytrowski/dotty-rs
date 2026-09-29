/// Parser-to-scanner feedback events used by Scala 3 layout handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannerEvent {
    ColonEol {
        in_template: bool,
    },
    Indented,
    /// Opens an indented region using a grammar-owned declaration/header
    /// offset rather than the physical line of the body introducer. This is
    /// needed when a multiline header ends on the same indentation column as
    /// its body.
    IndentedFrom {
        reference_offset: u32,
    },
    Outdented,
    /// Closes a specific active indentation region, if still active.
    OutdentedRegion {
        indent_offset: u32,
    },
    /// Opens the case region following a `match`, including same-indent cases
    /// in braced scopes where eager scanner layout is suppressed.
    MatchCasesIndented,
    /// Closes a parser-requested `match` case region after its case clauses.
    MatchCasesOutdented,
    /// Opens a case body whose statement indentation is relative to the case
    /// clause, not merely to its first expression.
    CaseBodyIndented {
        case_start: u32,
    },
    /// Closes a parser-requested indentation region using a grammar delimiter
    /// without exposing an `Outdent` token to the parser.
    OutdentedByDelimiter,
    ArrowIndented,
    SelfArrow,
}
