//! Structured diagnostics emitted by the Scala lexer.

use dotty_core::diagnostics::Diagnostic;

/// Structured failures reported while lexing Scala source.
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
    /// A failure while scanning XML syntax or recovering XML state.
    Xml(XmlIssue),
    /// The lexer does not currently support this source character.
    UnsupportedSourceCharacter { character: char },
    /// A block comment reached end of input before it was closed.
    UnterminatedBlockComment,
    /// A backquoted identifier reached end of input before its closing quote.
    UnterminatedBackquotedIdentifier,
    /// A backquoted identifier was empty or consisted only of `_`.
    EmptyOrUnderscoreBackquotedIdentifier,
    /// Two indentation prefixes made of spaces and tabs that are incomparable
    /// by prefix ordering.
    IncompatibleIndentationPrefixes,
}

impl LexerIssue {
    /// Returns the stable, text-independent identifier for this diagnostic.
    ///
    /// Codes are suitable for machine classification and are independent of
    /// diagnostic wording, source spans, and Rust variant names.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Numeric(issue) => issue.code(),
            Self::Literal(issue) => issue.code(),
            Self::Escape(issue) => issue.code(),
            Self::Interpolation(issue) => issue.code(),
            Self::Xml(issue) => issue.code(),
            Self::UnsupportedSourceCharacter { .. } => "lexer.unsupported_source_character",
            Self::UnterminatedBlockComment => "lexer.unterminated_block_comment",
            Self::UnterminatedBackquotedIdentifier => "lexer.unterminated_backquoted_identifier",
            Self::EmptyOrUnderscoreBackquotedIdentifier => {
                "lexer.empty_or_underscore_backquoted_identifier"
            }
            Self::IncompatibleIndentationPrefixes => "lexer.incompatible_indentation_prefixes",
        }
    }
}

impl NumericIssue {
    const fn code(&self) -> &'static str {
        match self {
            Self::MissingDigits { .. } => "lexer.numeric.missing_digits",
            Self::InvalidDigitForBase { .. } => "lexer.numeric.invalid_digit_for_base",
            Self::MisplacedSeparator => "lexer.numeric.misplaced_separator",
            Self::TrailingSeparator => "lexer.numeric.trailing_separator",
            Self::LongSuffixOnNonInteger { .. } => "lexer.numeric.long_suffix_on_non_integer",
            Self::InvalidNumber => "lexer.numeric.invalid_number",
        }
    }
}

impl LiteralIssue {
    const fn code(&self) -> &'static str {
        match self {
            Self::UnterminatedCharacter => "lexer.literal.unterminated_character",
            Self::EmptyCharacter => "lexer.literal.empty_character",
            Self::CharacterOutsideUtf16CodeUnit { .. } => {
                "lexer.literal.character_outside_utf16_code_unit"
            }
            Self::MultipleCharacters => "lexer.literal.multiple_characters",
            Self::UnclosedString { .. } => "lexer.literal.unclosed_string",
        }
    }
}

impl EscapeIssue {
    const fn code(&self) -> &'static str {
        match self {
            Self::InvalidEscapeCharacter { .. } => "lexer.escape.invalid_escape_character",
            Self::UnterminatedEscape => "lexer.escape.unterminated_escape",
            Self::InvalidUnicodeEscapeCharacter { .. } => {
                "lexer.escape.invalid_unicode_escape_character"
            }
            Self::IncompleteUnicodeEscape => "lexer.escape.incomplete_unicode_escape",
            Self::UnsupportedOctalEscape { .. } => "lexer.escape.unsupported_octal_escape",
        }
    }
}

impl InterpolationIssue {
    const fn code(&self) -> &'static str {
        match self {
            Self::UnterminatedExpression => "lexer.interpolation.unterminated_expression",
            Self::UnclosedString { .. } => "lexer.interpolation.unclosed_string",
            Self::InvalidSplice { .. } => "lexer.interpolation.invalid_splice",
            Self::IdentifierExpectedAfterDollar => {
                "lexer.interpolation.identifier_expected_after_dollar"
            }
        }
    }
}

impl XmlIssue {
    const fn code(&self) -> &'static str {
        match self {
            Self::UnterminatedComment => "lexer.xml.unterminated_comment",
            Self::UnterminatedCdata => "lexer.xml.unterminated_cdata",
            Self::AttributeExpressionMustBeClosed => {
                "lexer.xml.attribute_expression_must_be_closed"
            }
            Self::UnterminatedExpression => "lexer.xml.unterminated_expression",
            Self::AttributeNameMustBeFollowedByEquals => {
                "lexer.xml.attribute_name_must_be_followed_by_equals"
            }
            Self::AttributeValueExpectedAfterEquals => {
                "lexer.xml.attribute_value_expected_after_equals"
            }
            Self::UnterminatedDeclaration => "lexer.xml.unterminated_declaration",
            Self::UnterminatedTag => "lexer.xml.unterminated_tag",
            Self::AttributeValueMustFollowEquals => "lexer.xml.attribute_value_must_follow_equals",
            Self::AttributeNameExpected => "lexer.xml.attribute_name_expected",
            Self::ClosingTagCannotContainAttributes => {
                "lexer.xml.closing_tag_cannot_contain_attributes"
            }
            Self::OpeningTagNameExpected => "lexer.xml.opening_tag_name_expected",
            Self::ClosingTagNameExpected => "lexer.xml.closing_tag_name_expected",
            Self::MismatchedClosingTag { .. } => "lexer.xml.mismatched_closing_tag",
            Self::ClosingTagWithoutOpeningTag { .. } => "lexer.xml.closing_tag_without_opening_tag",
            Self::InvalidCommentDoubleHyphen => "lexer.xml.invalid_comment_double_hyphen",
            Self::UnexpectedAttributeEquals => "lexer.xml.unexpected_attribute_equals",
        }
    }
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

/// Structured failures produced while tracking XML syntax and recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlIssue {
    /// An XML comment reached end of input before its terminator.
    UnterminatedComment,
    /// A CDATA section reached end of input before its terminator.
    UnterminatedCdata,
    /// An XML attribute expression reached end of input before its closing brace.
    AttributeExpressionMustBeClosed,
    /// An XML expression reached end of input before its closing brace.
    UnterminatedExpression,
    /// An attribute name was not followed by `=`.
    AttributeNameMustBeFollowedByEquals,
    /// An attribute assignment did not provide a value.
    AttributeValueExpectedAfterEquals,
    /// An XML declaration reached end of input before it was closed.
    UnterminatedDeclaration,
    /// An XML tag or containing element reached end of input before closing.
    UnterminatedTag,
    /// An attribute value was present without a preceding `=`.
    AttributeValueMustFollowEquals,
    /// An attribute name was expected in an opening tag.
    AttributeNameExpected,
    /// A closing tag contained attribute-like tokens.
    ClosingTagCannotContainAttributes,
    /// An opening tag began without a name.
    OpeningTagNameExpected,
    /// An XML closing tag began without a name.
    ClosingTagNameExpected,
    /// A closing tag name did not match the corresponding opening tag.
    MismatchedClosingTag { expected: String, found: String },
    /// A closing tag had no matching opening tag.
    ClosingTagWithoutOpeningTag { found: Option<String> },
    /// An XML comment contained the forbidden `--` sequence.
    InvalidCommentDoubleHyphen,
    /// An `=` token occurred where no attribute assignment was expected.
    UnexpectedAttributeEquals,
}

/// A diagnostic reported by the raw lexer or contextual scanner.
pub type LexerDiagnostic = Diagnostic<LexerIssue>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn issue_codes_are_stable_and_unique_by_diagnostic_kind() {
        let issues = [
            LexerIssue::Numeric(NumericIssue::MissingDigits {
                component: NumericComponent::BasePrefix { base: 16 },
            }),
            LexerIssue::Numeric(NumericIssue::InvalidDigitForBase {
                digit: '2',
                base: 2,
            }),
            LexerIssue::Numeric(NumericIssue::MisplacedSeparator),
            LexerIssue::Numeric(NumericIssue::TrailingSeparator),
            LexerIssue::Numeric(NumericIssue::LongSuffixOnNonInteger {
                literal_kind: NonIntegerLiteralKind::Decimal,
            }),
            LexerIssue::Numeric(NumericIssue::InvalidNumber),
            LexerIssue::Literal(LiteralIssue::UnterminatedCharacter),
            LexerIssue::Literal(LiteralIssue::EmptyCharacter),
            LexerIssue::Literal(LiteralIssue::CharacterOutsideUtf16CodeUnit { character: '𐐀' }),
            LexerIssue::Literal(LiteralIssue::MultipleCharacters),
            LexerIssue::Literal(LiteralIssue::UnclosedString { multiline: false }),
            LexerIssue::Escape(EscapeIssue::InvalidEscapeCharacter { character: 'q' }),
            LexerIssue::Escape(EscapeIssue::UnterminatedEscape),
            LexerIssue::Escape(EscapeIssue::InvalidUnicodeEscapeCharacter { character: 'g' }),
            LexerIssue::Escape(EscapeIssue::IncompleteUnicodeEscape),
            LexerIssue::Escape(EscapeIssue::UnsupportedOctalEscape {
                digits: "12".to_owned(),
            }),
            LexerIssue::Interpolation(InterpolationIssue::UnterminatedExpression),
            LexerIssue::Interpolation(InterpolationIssue::UnclosedString { multiline: false }),
            LexerIssue::Interpolation(InterpolationIssue::InvalidSplice {
                following_character: Some('!'),
            }),
            LexerIssue::Interpolation(InterpolationIssue::IdentifierExpectedAfterDollar),
            LexerIssue::Xml(XmlIssue::UnterminatedComment),
            LexerIssue::Xml(XmlIssue::UnterminatedCdata),
            LexerIssue::Xml(XmlIssue::AttributeExpressionMustBeClosed),
            LexerIssue::Xml(XmlIssue::UnterminatedExpression),
            LexerIssue::Xml(XmlIssue::AttributeNameMustBeFollowedByEquals),
            LexerIssue::Xml(XmlIssue::AttributeValueExpectedAfterEquals),
            LexerIssue::Xml(XmlIssue::UnterminatedDeclaration),
            LexerIssue::Xml(XmlIssue::UnterminatedTag),
            LexerIssue::Xml(XmlIssue::AttributeValueMustFollowEquals),
            LexerIssue::Xml(XmlIssue::AttributeNameExpected),
            LexerIssue::Xml(XmlIssue::ClosingTagCannotContainAttributes),
            LexerIssue::Xml(XmlIssue::OpeningTagNameExpected),
            LexerIssue::Xml(XmlIssue::ClosingTagNameExpected),
            LexerIssue::Xml(XmlIssue::MismatchedClosingTag {
                expected: "root".to_owned(),
                found: "other".to_owned(),
            }),
            LexerIssue::Xml(XmlIssue::ClosingTagWithoutOpeningTag {
                found: Some("orphan".to_owned()),
            }),
            LexerIssue::Xml(XmlIssue::InvalidCommentDoubleHyphen),
            LexerIssue::Xml(XmlIssue::UnexpectedAttributeEquals),
            LexerIssue::UnsupportedSourceCharacter { character: '\0' },
            LexerIssue::UnterminatedBlockComment,
            LexerIssue::UnterminatedBackquotedIdentifier,
            LexerIssue::EmptyOrUnderscoreBackquotedIdentifier,
            LexerIssue::IncompatibleIndentationPrefixes,
        ];

        let codes = issues.iter().map(LexerIssue::code).collect::<Vec<_>>();
        let unique_codes = codes.iter().copied().collect::<BTreeSet<_>>();
        let expected_codes = [
            "lexer.numeric.missing_digits",
            "lexer.numeric.invalid_digit_for_base",
            "lexer.numeric.misplaced_separator",
            "lexer.numeric.trailing_separator",
            "lexer.numeric.long_suffix_on_non_integer",
            "lexer.numeric.invalid_number",
            "lexer.literal.unterminated_character",
            "lexer.literal.empty_character",
            "lexer.literal.character_outside_utf16_code_unit",
            "lexer.literal.multiple_characters",
            "lexer.literal.unclosed_string",
            "lexer.escape.invalid_escape_character",
            "lexer.escape.unterminated_escape",
            "lexer.escape.invalid_unicode_escape_character",
            "lexer.escape.incomplete_unicode_escape",
            "lexer.escape.unsupported_octal_escape",
            "lexer.interpolation.unterminated_expression",
            "lexer.interpolation.unclosed_string",
            "lexer.interpolation.invalid_splice",
            "lexer.interpolation.identifier_expected_after_dollar",
            "lexer.xml.unterminated_comment",
            "lexer.xml.unterminated_cdata",
            "lexer.xml.attribute_expression_must_be_closed",
            "lexer.xml.unterminated_expression",
            "lexer.xml.attribute_name_must_be_followed_by_equals",
            "lexer.xml.attribute_value_expected_after_equals",
            "lexer.xml.unterminated_declaration",
            "lexer.xml.unterminated_tag",
            "lexer.xml.attribute_value_must_follow_equals",
            "lexer.xml.attribute_name_expected",
            "lexer.xml.closing_tag_cannot_contain_attributes",
            "lexer.xml.opening_tag_name_expected",
            "lexer.xml.closing_tag_name_expected",
            "lexer.xml.mismatched_closing_tag",
            "lexer.xml.closing_tag_without_opening_tag",
            "lexer.xml.invalid_comment_double_hyphen",
            "lexer.xml.unexpected_attribute_equals",
            "lexer.unsupported_source_character",
            "lexer.unterminated_block_comment",
            "lexer.unterminated_backquoted_identifier",
            "lexer.empty_or_underscore_backquoted_identifier",
            "lexer.incompatible_indentation_prefixes",
        ];

        assert_eq!(codes, expected_codes);
        assert_eq!(unique_codes.len(), codes.len());
    }

    #[test]
    fn issue_codes_do_not_depend_on_payload_values() {
        assert_eq!(
            LexerIssue::Xml(XmlIssue::MismatchedClosingTag {
                expected: "left".to_owned(),
                found: "right".to_owned(),
            })
            .code(),
            LexerIssue::Xml(XmlIssue::MismatchedClosingTag {
                expected: "outer".to_owned(),
                found: "inner".to_owned(),
            })
            .code()
        );
        assert_eq!(
            LexerIssue::Numeric(NumericIssue::MissingDigits {
                component: NumericComponent::BasePrefix { base: 2 },
            })
            .code(),
            LexerIssue::Numeric(NumericIssue::MissingDigits {
                component: NumericComponent::Exponent,
            })
            .code()
        );
    }
}
