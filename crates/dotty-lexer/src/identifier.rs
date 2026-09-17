/// Returns whether a character may start a Scala identifier.
pub(crate) fn is_identifier_start(character: char) -> bool {
    character == '_' || character == '$' || character.is_alphabetic()
}

/// Returns whether a character may continue a Scala identifier.
pub(crate) fn is_identifier_part(character: char) -> bool {
    is_identifier_start(character)
        || character.is_numeric()
        || unicode_ident::is_xid_continue(character)
}

/// Returns whether a character belongs to a Scala operator lexeme.
pub(crate) fn is_operator_character(character: char) -> bool {
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
    ) || is_unicode_symbol(character)
}

fn is_unicode_symbol(character: char) -> bool {
    let code = character as u32;

    matches!(
        code,
        0x2190..=0x21ff
            | 0x2200..=0x22ff
            | 0x2300..=0x23ff
            | 0x2500..=0x27bf
            | 0x2900..=0x2bff
            | 0x1d400..=0x1d7ff
            | 0x1f000..=0x1faff
            | 0x2ff0..=0x303f
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff65
    )
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
    fn accepts_combining_marks_in_identifier_parts() {
        assert!(is_identifier_part('\u{0301}'));
    }

    #[test]
    fn recognizes_ascii_and_unicode_operator_characters() {
        assert!(is_operator_character('+'));
        assert!(is_operator_character('⇒'));
        assert!(is_operator_character('🂡'));
        assert!(!is_operator_character('a'));
    }
}
