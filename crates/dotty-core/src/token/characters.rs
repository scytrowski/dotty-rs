//! Character classes shared by Scala tokenization and operator precedence.

use unicode_general_category::{GeneralCategory::*, get_general_category};

fn scala_letter_category(category: unicode_general_category::GeneralCategory) -> bool {
    matches!(
        category,
        UppercaseLetter | LowercaseLetter | TitlecaseLetter | OtherLetter | LetterNumber
    )
}

/// Returns whether Scala gives an operator starting with this character letter precedence.
pub fn is_scala_letter(ch: char) -> bool {
    if ch.is_ascii() {
        return ch.is_ascii_alphabetic() || matches!(ch, '_' | '$');
    }
    scala_letter_category(get_general_category(ch))
}

fn is_unicode_start(ch: char, category: unicode_general_category::GeneralCategory) -> bool {
    scala_letter_category(category)
        || category == ModifierLetter
        || matches!(
            ch,
            '\u{1885}' | '\u{1886}' | '℘' | '℮' | '\u{309b}' | '\u{309c}'
        )
}

/// Returns whether a character may start a Scala identifier.
pub fn is_identifier_start(ch: char) -> bool {
    if ch.is_ascii() {
        return ch.is_ascii_alphabetic() || matches!(ch, '_' | '$');
    }
    is_unicode_start(ch, get_general_category(ch))
}

/// Returns whether a character may continue a Scala identifier.
pub fn is_identifier_part(ch: char) -> bool {
    // Scala's scanner explicitly stops before SU even though the JDK
    // includes it among ignorable controls in a Unicode identifier.
    if ch.is_ascii() {
        return ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '$' | '\0'..='\u{8}' | '\u{e}'..='\u{19}' | '\u{1b}' | '\u{7f}');
    }
    let category = get_general_category(ch);
    is_unicode_start(ch, category)
        || matches!(category, DecimalNumber | ConnectorPunctuation | SpacingMark | NonspacingMark | Format)
        // Other_ID_Continue and non-ASCII ignorable controls.
        || matches!(ch, '\u{80}'..='\u{9f}' | '·' | '\u{387}' | '\u{1369}'..='\u{1371}' | '\u{19da}' | '\u{30fb}' | '\u{ff65}')
}

/// Returns whether a character belongs to a Scala operator lexeme.
pub fn is_operator_character(character: char) -> bool {
    matches!(
        character,
        '!' | '#'
            | '%'
            | '&'
            | '*'
            | '+'
            | '-'
            | '/'
            | ':'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '\\'
            | '^'
            | '|'
            | '~'
    ) || (!character.is_ascii()
        && matches!(get_general_category(character), MathSymbol | OtherSymbol))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_and_unicode_identifier_starts() {
        assert!(is_identifier_start('_'));
        assert!(is_identifier_start('$'));
        assert!(is_identifier_start('α'));
        assert!(!is_identifier_start('7'));
    }

    #[test]
    fn accepts_digits_only_after_an_identifier_start() {
        assert!(is_identifier_part('7'));
        assert!(is_identifier_part('ż'));
        assert!(!is_identifier_part('+'));
    }

    #[test]
    fn distinguishes_ignorable_identifier_controls_from_separators() {
        assert!(is_identifier_part('\0'));
        assert!(is_identifier_part('\u{1b}'));
        assert!(!is_identifier_part('\u{1a}'));
        assert!(!is_identifier_part('\u{1c}'));
    }

    #[test]
    fn accepts_combining_marks_in_identifier_parts() {
        assert!(is_identifier_part('\u{0301}'));
    }

    #[test]
    fn recognizes_ascii_and_unicode_operator_characters() {
        assert!(is_operator_character('+'));
        assert!(is_operator_character('⇒'));
        assert!(is_operator_character('©'));
        assert!(is_operator_character('🂡'));
        assert!(!is_operator_character('a'));
    }
}
