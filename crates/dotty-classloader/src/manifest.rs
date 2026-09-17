/// Reads exactly one fact out of a JAR's `META-INF/MANIFEST.MF` bytes:
/// whether its main section declares `Multi-Release: true` (the JAR File
/// Specification's "Multi-release JAR files" mechanism, JEP 238).
///
/// Not a general manifest-attribute API — nothing else in this crate
/// needs one. `bytes` is decoded as UTF-8 best-effort
/// (`String::from_utf8_lossy`) rather than erroring on malformed
/// encoding: an unreadable manifest simply fails to match, the same
/// outcome as an absent attribute, so there is no separate error case
/// worth modeling.
///
/// Continuation lines (a line starting with a single space, folding a
/// header value that would otherwise exceed 72 bytes per the manifest
/// spec) are unfolded before matching, so a wrapped value is still read
/// correctly.
pub(crate) fn declares_multi_release(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);

    let mut unfolded_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(continuation) = line.strip_prefix(' ') {
            if let Some(last) = unfolded_lines.last_mut() {
                last.push_str(continuation);
                continue;
            }
        }
        unfolded_lines.push(line.to_owned());
    }

    unfolded_lines.iter().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("multi-release")
            && value.trim().eq_ignore_ascii_case("true")
    })
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
    fn returns_false_for_empty_input() {
        assert!(!declares_multi_release(b""));
    }
}
