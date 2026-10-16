//! Scala 3.9.0 artifact path components, separate from source/wire names.
//!
//! Mirrors the pinned compiler's util/NameTransformer.scala and SimpleName.encode
//! in core/Names.scala. Java identifier classification is frozen to the JDK 21
//! oracle toolchain; Scala iterates UTF-16 code units, including surrogate pairs.

mod java_identifier_part;

/// Encode one already validated source-name component, never an entire path.
pub(super) fn encode_component(name: &str) -> String {
    if matches!(name, "<init>" | "<clinit>") {
        return name.into();
    }
    let mut encoded = String::new();
    for unit in name.encode_utf16() {
        let operator = match unit {
            0x7E => Some("$tilde"),
            0x3D => Some("$eq"),
            0x3C => Some("$less"),
            0x3E => Some("$greater"),
            0x21 => Some("$bang"),
            0x23 => Some("$hash"),
            0x25 => Some("$percent"),
            0x5E => Some("$up"),
            0x26 => Some("$amp"),
            0x7C => Some("$bar"),
            0x2A => Some("$times"),
            0x2F => Some("$div"),
            0x2B => Some("$plus"),
            0x2D => Some("$minus"),
            0x3A => Some("$colon"),
            0x5C => Some("$bslash"),
            0x3F => Some("$qmark"),
            0x40 => Some("$at"),
            _ => None,
        };
        if let Some(operator) = operator {
            encoded.push_str(operator);
        } else if let Some(character) =
            char::from_u32(u32::from(unit)).filter(|_| java_identifier_part::contains(unit))
        {
            // No supplementary scalar is collapsed: each surrogate is escaped.
            encoded.push(character);
        } else {
            encoded.push_str("$u");
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            for shift in [12, 8, 4, 0] {
                encoded.push(HEX[usize::from((unit >> shift) & 0xF)] as char);
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_encodings_match_the_pinned_scala_table() {
        assert_eq!(
            encode_component("~=<>!#%^&|*/+-:\\?@"),
            "$tilde$eq$less$greater$bang$hash$percent$up$amp$bar$times$div$plus$minus$colon$bslash$qmark$at"
        );
        assert_eq!(encode_component("A-B"), "A$minusB");
        assert_eq!(encode_component("p+q"), "p$plusq");
        assert_eq!(
            encode_component("already$minusEncoded_42"),
            "already$minusEncoded_42"
        );
    }

    #[test]
    fn unicode_escaping_uses_java_identifier_parts_and_utf16_units() {
        for (source, expected) in [
            ("日本語", "日本語"),
            ("δοκιμή", "δοκιμή"),
            ("A B", "A$u0020B"),
            ("A😀", "A$uD83D$uDE00"),
            ("A𐐀", "A$uD801$uDC00"),
            ("A²", "A$u00B2"),
            ("Aⓐ", "A$u24D0"),
            ("a\u{0301}", "a\u{0301}"),
            ("€name", "€name"),
        ] {
            assert_eq!(encode_component(source), expected, "{source}");
        }
    }

    #[test]
    fn special_constructor_names_are_not_encoded_by_simple_name_encode() {
        assert_eq!(encode_component("<init>"), "<init>");
        assert_eq!(encode_component("<clinit>"), "<clinit>");
    }
}
