//! Structured diagnostics emitted by the Scala lexer.

use dotty_core::diagnostics::Diagnostic;

/// A lexer diagnostic that has not yet been migrated to a structured variant.
///
/// This is a transitional payload. New lexer diagnostics should use a typed
/// variant whenever their information has a stable structured representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexerIssue {
    /// A failure while recognizing a numeric literal.
    Numeric(NumericIssue),
    /// A failure while recognizing a character or string literal.
    Literal(LiteralIssue),
    /// A failure while decoding a literal escape.
    Escape(EscapeIssue),
    /// A failure while scanning an interpolated string or splice.
    Interpolation(InterpolationIssue),
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

/// Structured failures produced while scanning numeric literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericIssue {
    /// A base prefix, fractional part, or exponent had no digits.
    MissingDigits { component: NumericComponent },
    /// A digit was not valid for the selected non-decimal base.
    InvalidDigitForBase { digit: char, base: u32 },
    /// A separator occurred before any digit or directly after another one.
    MisplacedSeparator,
    /// A numeric component ended in a separator.
    TrailingSeparator,
    /// The `L` suffix was applied to a decimal or exponent literal.
    LongSuffixOnNonInteger { literal_kind: NonIntegerLiteralKind },
    /// The numeric portion was followed by an invalid identifier character.
    InvalidNumber,
}

/// The part of a number that was expected to contain at least one digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericComponent {
    /// Digits after a `0x` or `0b` prefix.
    BasePrefix { base: u32 },
    /// Digits after the decimal point.
    FractionalPart,
    /// Digits after the exponent marker and optional sign.
    Exponent,
}

/// A decimal numeric form that cannot take the `L` integer suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonIntegerLiteralKind {
    /// A decimal literal with a fractional part.
    Decimal,
    /// A literal with an exponent part.
    Exponent,
    /// A non-decimal or otherwise unclassified numeric form.
    Other,
}

/// Structured failures produced while scanning character and string literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralIssue {
    /// A character literal ended before its closing quote.
    UnterminatedCharacter,
    /// A character literal contained no character.
    EmptyCharacter,
    /// The source character occupies more than one JVM UTF-16 code unit.
    CharacterOutsideUtf16CodeUnit { character: char },
    /// A character literal contained multiple characters.
    MultipleCharacters,
    /// An ordinary or multiline string did not reach its closing delimiter.
    UnclosedString { multiline: bool },
}

/// Structured failures produced while scanning escapes in literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscapeIssue {
    /// An escape used a character not supported by Scala string syntax.
    InvalidEscapeCharacter { character: char },
    /// A backslash occurred at end of input without an escape character.
    UnterminatedEscape,
    /// A Unicode escape contained a non-hexadecimal character.
    InvalidUnicodeEscapeCharacter { character: char },
    /// A Unicode escape ended before all four hexadecimal digits were read.
    IncompleteUnicodeEscape,
    /// Legacy octal escapes are recognized but unsupported.
    UnsupportedOctalEscape { digits: String },
}

/// Structured failures produced while scanning interpolation syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolationIssue {
    /// A braced interpolation expression reached end of input before `}`.
    UnterminatedExpression,
    /// An interpolated string reached its end without a closing delimiter.
    UnclosedString { multiline: bool },
    /// A `$` was not followed by a valid simple or braced splice.
    InvalidSplice { following_character: Option<char> },
    /// A simple `$` splice was not followed by an identifier.
    IdentifierExpectedAfterDollar,
}

/// A diagnostic reported by the raw lexer or contextual scanner.
pub type LexerDiagnostic = Diagnostic<LexerIssue>;
