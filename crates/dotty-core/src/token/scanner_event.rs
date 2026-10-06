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
    /// Closes a specific parser-opened indentation region, if still active.
    OutdentedRegion {
        indent_offset: u32,
    },
    /// Closes the named innermost layout region, whether it was emitted by
    /// eager scanning or opened through parser feedback.
    OutdentedLayoutRegion {
        indent_offset: u32,
    },
    /// Opens the case region following a `match`, including same-indent cases
    /// in braced scopes where eager scanner layout is suppressed.
    MatchCasesIndented,
    /// Closes a parser-requested `match` case region after its case clauses.
    MatchCasesOutdented,
    /// Closes the named parser-requested `match` case region after its case
    /// clauses. The parser has already established that the next token is
    /// outside this match, even when it is aligned with the case clauses.
    MatchCasesClosed {
        indent_offset: u32,
    },
    /// Opens a case body whose statement indentation is relative to the case
    /// clause, not merely to its first expression.
    CaseBodyIndented {
        case_start: u32,
    },
    /// Opens the guard portion of a case clause, where operator continuations
    /// may cross a line at the case indentation.
    CaseClauseStarted {
        case_start: u32,
    },
    /// Closes the case-clause separator region after its optional guard.
    CaseClauseEnded,
    /// Closes a parser-requested indentation region using a grammar delimiter
    /// without exposing an `Outdent` token to the parser.
    OutdentedByDelimiter,
    /// Closes parser feedback for a region whose `Outdent` was already present
    /// in the token stream and consumed by the parser.
    OutdentedByExistingOutdent {
        indent_offset: u32,
    },
    ArrowIndented,
    SelfArrow,
}
