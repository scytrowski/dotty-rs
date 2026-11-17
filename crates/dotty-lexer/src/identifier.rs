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
}
