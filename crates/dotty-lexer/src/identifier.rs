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

    // Generated from JDK 25 Character.MATH_SYMBOL/OTHER_SYMBOL ranges.
    matches!(
        code,
        0x2b
            | 0x3c..=0x3e
            | 0x7c
            | 0x7e
            | 0xa6
            | 0xa9
            | 0xac
            | 0xae
            | 0xb0..=0xb1
            | 0xd7
            | 0xf7
            | 0x3f6
            | 0x482
            | 0x58d..=0x58e
            | 0x606..=0x608
            | 0x60e..=0x60f
            | 0x6de
            | 0x6e9
            | 0x6fd..=0x6fe
            | 0x7f6
            | 0x9fa
            | 0xb70
            | 0xbf3..=0xbf8
            | 0xbfa
            | 0xc7f
            | 0xd4f
            | 0xd79
            | 0xf01..=0xf03
            | 0xf13
            | 0xf15..=0xf17
            | 0xf1a..=0xf1f
            | 0xf34
            | 0xf36
            | 0xf38
            | 0xfbe..=0xfc5
            | 0xfc7..=0xfcc
            | 0xfce..=0xfcf
            | 0xfd5..=0xfd8
            | 0x109e..=0x109f
            | 0x1390..=0x1399
            | 0x166d
            | 0x1940
            | 0x19de..=0x19ff
            | 0x1b61..=0x1b6a
            | 0x1b74..=0x1b7c
            | 0x2044
            | 0x2052
            | 0x207a..=0x207c
            | 0x208a..=0x208c
            | 0x2100..=0x2101
            | 0x2103..=0x2106
            | 0x2108..=0x2109
            | 0x2114
            | 0x2116..=0x2118
            | 0x211e..=0x2123
            | 0x2125
            | 0x2127
            | 0x2129
            | 0x212e
            | 0x213a..=0x213b
            | 0x2140..=0x2144
            | 0x214a..=0x214d
            | 0x214f
            | 0x218a..=0x218b
            | 0x2190..=0x2307
            | 0x230c..=0x2328
            | 0x232b..=0x2429
            | 0x2440..=0x244a
            | 0x249c..=0x24e9
            | 0x2500..=0x2767
            | 0x2794..=0x27c4
            | 0x27c7..=0x27e5
            | 0x27f0..=0x2982
            | 0x2999..=0x29d7
            | 0x29dc..=0x29fb
            | 0x29fe..=0x2b73
            | 0x2b76..=0x2b95
            | 0x2b97..=0x2bff
            | 0x2ce5..=0x2cea
            | 0x2e50..=0x2e51
            | 0x2e80..=0x2e99
            | 0x2e9b..=0x2ef3
            | 0x2f00..=0x2fd5
            | 0x2ff0..=0x2fff
            | 0x3004
            | 0x3012..=0x3013
            | 0x3020
            | 0x3036..=0x3037
            | 0x303e..=0x303f
            | 0x3190..=0x3191
            | 0x3196..=0x319f
            | 0x31c0..=0x31e5
            | 0x31ef
            | 0x3200..=0x321e
            | 0x322a..=0x3247
            | 0x3250
            | 0x3260..=0x327f
            | 0x328a..=0x32b0
            | 0x32c0..=0x33ff
            | 0x4dc0..=0x4dff
            | 0xa490..=0xa4c6
            | 0xa828..=0xa82b
            | 0xa836..=0xa837
            | 0xa839
            | 0xaa77..=0xaa79
            | 0xfb29
            | 0xfd40..=0xfd4f
            | 0xfdcf
            | 0xfdfd..=0xfdff
            | 0xfe62
            | 0xfe64..=0xfe66
            | 0xff0b
            | 0xff1c..=0xff1e
            | 0xff5c
            | 0xff5e
            | 0xffe2
            | 0xffe4
            | 0xffe8..=0xffee
            | 0xfffc..=0xfffd
            | 0x10137..=0x1013f
            | 0x10179..=0x10189
            | 0x1018c..=0x1018e
            | 0x10190..=0x1019c
            | 0x101a0
            | 0x101d0..=0x101fc
            | 0x10877..=0x10878
            | 0x10ac8
            | 0x10d8e..=0x10d8f
            | 0x1173f
            | 0x11fd5..=0x11fdc
            | 0x11fe1..=0x11ff1
            | 0x16b3c..=0x16b3f
            | 0x16b45
            | 0x1bc9c
            | 0x1cc00..=0x1ccef
            | 0x1cd00..=0x1ceb3
            | 0x1cf50..=0x1cfc3
            | 0x1d000..=0x1d0f5
            | 0x1d100..=0x1d126
            | 0x1d129..=0x1d164
            | 0x1d16a..=0x1d16c
            | 0x1d183..=0x1d184
            | 0x1d18c..=0x1d1a9
            | 0x1d1ae..=0x1d1ea
            | 0x1d200..=0x1d241
            | 0x1d245
            | 0x1d300..=0x1d356
            | 0x1d6c1
            | 0x1d6db
            | 0x1d6fb
            | 0x1d715
            | 0x1d735
            | 0x1d74f
            | 0x1d76f
            | 0x1d789
            | 0x1d7a9
            | 0x1d7c3
            | 0x1d800..=0x1d9ff
            | 0x1da37..=0x1da3a
            | 0x1da6d..=0x1da74
            | 0x1da76..=0x1da83
            | 0x1da85..=0x1da86
            | 0x1e14f
            | 0x1ecac
            | 0x1ed2e
            | 0x1eef0..=0x1eef1
            | 0x1f000..=0x1f02b
            | 0x1f030..=0x1f093
            | 0x1f0a0..=0x1f0ae
            | 0x1f0b1..=0x1f0bf
            | 0x1f0c1..=0x1f0cf
            | 0x1f0d1..=0x1f0f5
            | 0x1f10d..=0x1f1ad
            | 0x1f1e6..=0x1f202
            | 0x1f210..=0x1f23b
            | 0x1f240..=0x1f248
            | 0x1f250..=0x1f251
            | 0x1f260..=0x1f265
            | 0x1f300..=0x1f3fa
            | 0x1f400..=0x1f6d7
            | 0x1f6dc..=0x1f6ec
            | 0x1f6f0..=0x1f6fc
            | 0x1f700..=0x1f776
            | 0x1f77b..=0x1f7d9
            | 0x1f7e0..=0x1f7eb
            | 0x1f7f0
            | 0x1f800..=0x1f80b
            | 0x1f810..=0x1f847
            | 0x1f850..=0x1f859
            | 0x1f860..=0x1f887
            | 0x1f890..=0x1f8ad
            | 0x1f8b0..=0x1f8bb
            | 0x1f8c0..=0x1f8c1
            | 0x1f900..=0x1fa53
            | 0x1fa60..=0x1fa6d
            | 0x1fa70..=0x1fa7c
            | 0x1fa80..=0x1fa89
            | 0x1fa8f..=0x1fac6
            | 0x1face..=0x1fadc
            | 0x1fadf..=0x1fae9
            | 0x1faf0..=0x1faf8
            | 0x1fb00..=0x1fb92
            | 0x1fb94..=0x1fbef
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
        assert!(is_operator_character('©'));
        assert!(is_operator_character('🂡'));
        assert!(!is_operator_character('a'));
    }
}
