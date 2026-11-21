use crate::lexer::{
    EscapeIssue, InterpolationIssue, LexerIssue, LiteralIssue, NonIntegerLiteralKind,
    NumericComponent, NumericIssue, XmlIssue,
};

pub(super) fn message(issue: &LexerIssue) -> String {
    match issue {
        LexerIssue::Numeric(issue) => numeric_message(issue),
        LexerIssue::Literal(issue) => literal_message(issue),
        LexerIssue::Escape(issue) => escape_message(issue),
        LexerIssue::UnterminatedBlockComment => "Unterminated block comment".into(),
        LexerIssue::Interpolation(issue) => interpolation_message(issue),
        LexerIssue::UnsupportedSourceCharacter { character } => {
            format!("Unsupported source character {character:?}")
        }
        LexerIssue::UnterminatedBackquotedIdentifier => {
            "Expected a closing backquote for the identifier".into()
        }
        LexerIssue::EmptyOrUnderscoreBackquotedIdentifier => {
            "A backquoted identifier cannot be empty or contain only '_'".into()
        }
        LexerIssue::NonScalarBackquotedIdentifier => {
            "Unpaired Unicode surrogate in backquoted identifier".into()
        }
        LexerIssue::IncompatibleIndentationPrefixes => {
            "Inconsistent indentation: spaces and tabs give incompatible prefixes".into()
        }
        LexerIssue::Xml(issue) => xml_message(issue),
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

fn interpolation_message(issue: &InterpolationIssue) -> String {
    match issue {
        InterpolationIssue::UnterminatedExpression => {
            "Expected '}' to close the interpolated expression".into()
        }
        InterpolationIssue::UnclosedString { multiline: false } => {
            "Unterminated interpolated string".into()
        }
        InterpolationIssue::UnclosedString { multiline: true } => {
            "Unterminated triple-quoted interpolated string".into()
        }
        InterpolationIssue::InvalidSplice {
            following_character: Some(character),
        } => format!("Expected an identifier or '{{' after '$', found {character:?}"),
        InterpolationIssue::InvalidSplice {
            following_character: None,
        } => "Expected an identifier or '{' after '$' at end of input".into(),
        InterpolationIssue::IdentifierExpectedAfterDollar => {
            "Expected an identifier after '$'".into()
        }
    }
}

fn xml_message(issue: &XmlIssue) -> String {
    match issue {
        XmlIssue::UnterminatedComment => "Expected '-->' to close the XML comment".into(),
        XmlIssue::UnterminatedCdata => "Expected ']]>' to close the CDATA section".into(),
        XmlIssue::AttributeExpressionMustBeClosed => {
            "Expected '}' to close the XML attribute expression".into()
        }
        XmlIssue::UnterminatedExpression => {
            "Expected '}' to close the embedded Scala expression in XML".into()
        }
        XmlIssue::AttributeNameMustBeFollowedByEquals => {
            "Expected '=' after the XML attribute name".into()
        }
        XmlIssue::AttributeValueExpectedAfterEquals => {
            "Expected an XML attribute value after '='".into()
        }
        XmlIssue::UnterminatedDeclaration => "Unterminated XML declaration".into(),
        XmlIssue::UnterminatedTag => "Unterminated XML tag or element".into(),
        XmlIssue::AttributeValueMustFollowEquals => {
            "Expected '=' before the XML attribute value".into()
        }
        XmlIssue::AttributeNameExpected => "Expected an XML attribute name".into(),
        XmlIssue::ClosingTagCannotContainAttributes => {
            "An XML closing tag cannot have attributes".into()
        }
        XmlIssue::OpeningTagNameExpected => {
            "Expected an element name in the XML opening tag".into()
        }
        XmlIssue::ClosingTagNameExpected => {
            "Expected an element name in the XML closing tag".into()
        }
        XmlIssue::MismatchedClosingTag { expected, found } => {
            format!("Expected </{expected}>, found </{found}>")
        }
        XmlIssue::ClosingTagWithoutOpeningTag { found: Some(name) } => {
            format!("No matching opening tag for </{name}>")
        }
        XmlIssue::ClosingTagWithoutOpeningTag { found: None } => {
            "XML closing tag has no matching opening tag".into()
        }
        XmlIssue::InvalidCommentDoubleHyphen => "An XML comment cannot contain '--'".into(),
        XmlIssue::UnexpectedAttributeEquals => "Unexpected '=' in XML attribute list".into(),
    }
}
