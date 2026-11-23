/// Reads the Multi-Release attribute from the main section of a JAR manifest.
/// CRLF, LF and CR each terminate a physical line. A leading space continues
/// the preceding value, and the first empty line ends the main section.
pub(crate) fn declares_multi_release(mut bytes: &[u8]) -> bool {
    const HEADER: &[u8] = b"Multi-Release: ";
    let mut value = Vec::new();
    let mut reading_value = false;
    while !bytes.is_empty() {
        let end = bytes
            .iter()
            .position(|b| matches!(b, b'\r' | b'\n'))
            .unwrap_or(bytes.len());
        let (line, rest) = bytes.split_at(end);
        if line.is_empty() {
            break;
        }
        bytes = match rest {
            [b'\r', b'\n', tail @ ..] | [_, tail @ ..] => tail,
            [] => &[],
        };
        if let Some(continuation) = line.strip_prefix(b" ") {
            if reading_value {
                value.extend_from_slice(continuation);
            }
        } else {
            reading_value = line
                .get(..HEADER.len())
                .is_some_and(|name| name.eq_ignore_ascii_case(HEADER));
            if reading_value {
                value.clear();
                value.extend_from_slice(&line[HEADER.len()..]);
            }
        }
    }
    value.eq_ignore_ascii_case(b"true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_multi_release_true() {
        let manifest = b"Manifest-Version: 1.0\nMulti-Release: true\n";
        assert!(declares_multi_release(manifest));
    }

    #[test]
    fn returns_false_when_the_attribute_is_absent() {
        let manifest = b"Manifest-Version: 1.0\nCreated-By: 25 (Corretto)\n";
        assert!(!declares_multi_release(manifest));
    }

    #[test]
    fn returns_false_when_the_value_is_false() {
        let manifest = b"Manifest-Version: 1.0\nMulti-Release: false\n";
        assert!(!declares_multi_release(manifest));
    }

    #[test]
    fn matches_the_attribute_name_case_insensitively() {
        let manifest = b"Manifest-Version: 1.0\nmulti-release: true\n";
        assert!(declares_multi_release(manifest));
    }

    #[test]
    fn matches_the_value_case_insensitively() {
        let manifest = b"Manifest-Version: 1.0\nMulti-Release: TRUE\n";
        assert!(declares_multi_release(manifest));
    }

    #[test]
    fn unfolds_a_continuation_line_before_matching() {
        // A folded header ("Some-Other-Attribute") wraps onto a
        // continuation line (leading single space) before the
        // Multi-Release header; unfolding must not corrupt or skip the
        // header that follows it.
        let manifest =
            b"Manifest-Version: 1.0\nSome-Other-Attribute: a very long value that wra\n ps onto a second line\nMulti-Release: true\n";
        assert!(declares_multi_release(manifest));
    }

    #[test]
    fn only_the_main_section_can_enable_multi_release() {
        for newline in ["\r", "\n", "\r\n"] {
            let manifest = [
                "Manifest-Version: 1.0",
                "",
                "Name: Item.class",
                "Multi-Release: true",
                "",
            ]
            .join(newline);
            assert!(!declares_multi_release(manifest.as_bytes()), "{newline:?}");
        }
    }

    #[test]
    fn unfolds_the_main_attribute_across_all_manifest_line_endings() {
        for newline in ["\r", "\n", "\r\n"] {
            let manifest = [
                "Manifest-Version: 1.0",
                "Multi-Release: Tr",
                " Ue",
                "",
                "Name: Item.class",
                "Multi-Release: false",
            ]
            .join(newline);
            assert!(declares_multi_release(manifest.as_bytes()), "{newline:?}");
        }
    }

    #[test]
    fn returns_false_for_empty_input() {
        assert!(!declares_multi_release(b""));
    }
}
