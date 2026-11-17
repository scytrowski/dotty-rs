use std::borrow::Cow;

/// Decodes the body of an escaped Scala literal into JVM UTF-16 code units.
/// Delimiters are excluded. Diagnostic reporting belongs to the lexical consumer.
pub fn decode_literal_units(body: &str) -> Option<Vec<u16>> {
    let mut chars = body.chars().peekable();
    let mut units = Vec::new();
    while let Some(ch) = chars.next() {
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
                    unit = unit * 16 + chars.next()?.to_digit(16)? as u16;
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
            _ => return None,
        };
        units.push(unit);
    }
    Some(units)
}

/// Decodes the body of a backquoted name for the UTF-8 name interner.
pub fn decode_identifier_body(body: &str) -> Option<Cow<'_, str>> {
    if !body.contains('\\') {
        return Some(Cow::Borrowed(body));
    }
    String::from_utf16(&decode_literal_units(body)?)
        .ok()
        .map(Cow::Owned)
}
