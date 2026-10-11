//! Structured diagnostics emitted by the Scala lexer.

use dotty_core::diagnostics::Diagnostic;

/// A lexer diagnostic that has not yet been migrated to a structured variant.
///
/// This is a transitional payload. New lexer diagnostics should use a typed
/// variant whenever their information has a stable structured representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexerIssue {
    /// The lexer does not currently support this source character.
    UnsupportedSourceCharacter { character: char },
    /// A block comment reached end of input before it was closed.
    UnterminatedBlockComment,
    /// A backquoted identifier reached end of input before its closing quote.
    UnterminatedBackquotedIdentifier,
    /// A backquoted identifier was empty or consisted only of `_`.
    EmptyOrUnderscoreBackquotedIdentifier,
    /// A diagnostic whose structured representation has not been migrated.
    Legacy { message: String },
}

/// A diagnostic reported by the raw lexer or contextual scanner.
pub type LexerDiagnostic = Diagnostic<LexerIssue>;
