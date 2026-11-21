use crate::lexer::{
    EscapeIssue, LexerIssue, LiteralIssue, NonIntegerLiteralKind, NumericComponent, NumericIssue,
};

pub(super) fn message(issue: &LexerIssue) -> String {
    match issue {
        LexerIssue::Numeric(issue) => numeric_message(issue),
        LexerIssue::Literal(issue) => literal_message(issue),
        LexerIssue::Escape(issue) => escape_message(issue),
        LexerIssue::UnterminatedBlockComment => "Unterminated block comment".into(),
        _ => format!("Lexer diagnostic ({})", issue.code()),
    }
}

fn numeric_message(issue: &NumericIssue) -> String {
    match issue {
        NumericIssue::MissingDigits { component } => match component {
            NumericComponent::BasePrefix { base } => {
                format!("Expected a base-{base} digit after the prefix")
            }
            NumericComponent::FractionalPart => "Expected a digit after the decimal point".into(),
            NumericComponent::Exponent => "Expected a digit in the exponent".into(),
        },
        NumericIssue::InvalidDigitForBase { digit, base } => {
            format!("Invalid digit {digit:?} for base {base}")
        }
        NumericIssue::MisplacedSeparator => "Misplaced '_' in numeric literal".into(),
        NumericIssue::TrailingSeparator => {
            "Expected a digit after the final '_' in the number".into()
        }
        NumericIssue::LongSuffixOnNonInteger { literal_kind } => match literal_kind {
            NonIntegerLiteralKind::Decimal => {
                "A literal with a fractional part cannot have an 'L' suffix".into()
            }
            NonIntegerLiteralKind::Exponent => {
                "A literal with an exponent cannot have an 'L' suffix".into()
            }
            NonIntegerLiteralKind::Other => "Only integer literals can have an 'L' suffix".into(),
        },
        NumericIssue::InvalidNumber => {
            "Unexpected identifier character after numeric literal".into()
        }
    }
}

fn literal_message(issue: &LiteralIssue) -> String {
    match issue {
        LiteralIssue::UnterminatedCharacter => "Unterminated character literal".into(),
        LiteralIssue::EmptyCharacter => "Empty character literal".into(),
        LiteralIssue::CharacterOutsideUtf16CodeUnit { character } => {
            format!("Character {character:?} does not fit in a single UTF-16 code unit")
        }
        LiteralIssue::MultipleCharacters => "Too many characters in character literal".into(),
        LiteralIssue::UnclosedString { multiline: false } => "Unterminated string literal".into(),
        LiteralIssue::UnclosedString { multiline: true } => {
            "Unterminated triple-quoted string literal".into()
        }
    }
}

fn escape_message(issue: &EscapeIssue) -> String {
    match issue {
        EscapeIssue::InvalidEscapeCharacter { character } => {
            format!("Invalid escape character {character:?}")
        }
        EscapeIssue::UnterminatedEscape => {
            "Expected an escape character after the backslash".into()
        }
        EscapeIssue::InvalidUnicodeEscapeCharacter { character } => {
            format!("Expected a hexadecimal digit in Unicode escape, found {character:?}")
        }
        EscapeIssue::IncompleteUnicodeEscape => {
            "Expected four hexadecimal digits in Unicode escape".into()
        }
        EscapeIssue::UnsupportedOctalEscape { digits } => {
            format!("Unsupported octal escape '\\{digits}'")
        }
    }
}
