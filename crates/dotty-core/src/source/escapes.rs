use std::borrow::Cow;

/// Decodes the body of an escaped Scala literal into JVM UTF-16 code units.
/// Delimiters are excluded. Diagnostic reporting belongs to the lexical consumer.
pub fn decode_literal_units(body: &str) -> Option<Vec<u16>> {
    decode_units(body, false)
}

fn decode_units(body: &str, recover_name: bool) -> Option<Vec<u16>> {
    let mut chars = body.chars().peekable();
    let mut units = Vec::new();
    'body: while let Some(ch) = chars.next() {
        if ch != '\\' {
            units.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
            continue;
        }
        let unit = match chars.next()? {
            'b' => 8,
            't' => 9,
            'n' => 10,
            'f' => 12,
            'r' => 13,
            escaped @ ('\\' | '"' | '\'') => escaped as u16,
            'u' | 'U' => {
                while chars.next_if(|ch| matches!(ch, 'u' | 'U')).is_some() {}
                let mut unit = 0;
                for _ in 0..4 {
                    if let Some(digit) = chars.peek().and_then(|ch| ch.to_digit(16)) {
                        let _ = chars.next();
                        unit = unit * 16 + digit as u16;
                    } else if recover_name {
                        // Scala copies the offending UTF-16 unit, then scans it again.
                        // At the end of the body this unit is the closing backtick.
                        let ch = chars.peek().copied().unwrap_or('`');
                        units.push(ch.encode_utf16(&mut [0; 2])[0]);
                        continue 'body;
                    } else {
                        return None;
                    }
                }
                unit
            }
            lead @ '0'..='7' => {
                let mut unit = lead as u16 - b'0' as u16;
                let remaining = if lead <= '3' { 2 } else { 1 };
                for _ in 0..remaining {
                    let Some(digit) = chars.next_if(|ch| matches!(ch, '0'..='7')) else {
                        break;
                    };
                    unit = unit * 8 + (digit as u16 - b'0' as u16);
                }
                unit
            }
            escaped if recover_name => {
                units.extend_from_slice(escaped.encode_utf16(&mut [0; 2]));
                continue;
            }
            _ => return None,
        };
        units.push(unit);
    }
    Some(units)
}

/// Decodes the body of a backquoted name for the UTF-8 name interner.
/// Applies Scala name recovery after the scanner has diagnosed an invalid escape.
pub fn decode_identifier_body(body: &str) -> Option<Cow<'_, str>> {
    if !body.contains('\\') {
        return Some(Cow::Borrowed(body));
    }
    String::from_utf16(&decode_units(body, true)?)
        .ok()
        .map(Cow::Owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_diagnosed_escapes_in_names() {
        assert_eq!(decode_identifier_body(r"a\q").as_deref(), Some("aq"));
        assert_eq!(decode_identifier_body(r"a\u21Z").as_deref(), Some("aZZ"));
        assert_eq!(decode_identifier_body(r"a\u21").as_deref(), Some("a`"));
        assert!(decode_literal_units(r"a\q").is_none());
        assert!(decode_literal_units(r"a\u21Z").is_none());
    }
}
