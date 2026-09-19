use dotty_core::ast::{Literal, NumberKind, NumberLiteral, UntypedNode};
use dotty_core::{Constant, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_number(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let token_kind = self.current().kind;
        let spelling = match self.current_text() {
            Ok(spelling) => spelling,
            Err(_) => return self.unexpected_expression(),
        };

        let kind = match token_kind {
            TokenKind::LongLiteral => {
                let Some(value) = parse_long_literal(spelling) else {
                    return self.unexpected_expression();
                };
                self.advance();
                return self.alloc_from(
                    mark,
                    TreeKind::Literal(Literal {
                        value: Constant::Long(value),
                    }),
                );
            }
            TokenKind::FloatLiteral => {
                let Some(value) = parse_float_literal(spelling) else {
                    return self.unexpected_expression();
                };
                self.advance();
                return self.alloc_from(
                    mark,
                    TreeKind::Literal(Literal {
                        value: Constant::Float(value),
                    }),
                );
            }
            TokenKind::DoubleLiteral => {
                let Some(value) = parse_double_literal(spelling) else {
                    return self.unexpected_expression();
                };
                self.advance();
                return self.alloc_from(
                    mark,
                    TreeKind::Literal(Literal {
                        value: Constant::Double(value),
                    }),
                );
            }
            TokenKind::IntegerLiteral => NumberKind::Whole(integer_radix(Some(spelling))),
            TokenKind::DecimalLiteral => NumberKind::Decimal,
            TokenKind::ExponentLiteral => NumberKind::Floating,
            _ => return self.unexpected_expression(),
        };
        let text = self.names.intern(spelling);
        self.advance();
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Number(NumberLiteral { text, kind })),
        )
    }

    pub(crate) fn parse_string(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let Ok(text) = self.current_text() else {
            return self.unexpected_expression();
        };
        let Some(value) = decode_string_literal(text) else {
            return self.unexpected_expression();
        };
        let value = match value {
            DecodedString::Scalar(value) => Constant::String(self.names.intern(&value)),
            DecodedString::Utf16(units) => Constant::StringUtf16(units),
        };
        self.advance();
        self.alloc_from(mark, TreeKind::Literal(Literal { value }))
    }

    pub(crate) fn parse_literal(&mut self, mark: crate::Mark, value: Constant) -> TreeId<Untyped> {
        self.advance();
        self.alloc_from(mark, TreeKind::Literal(Literal { value }))
    }
}

pub(crate) fn integer_radix(text: Option<&str>) -> u32 {
    match text {
        Some(text) if text.starts_with("0x") || text.starts_with("0X") => 16,
        Some(text) if text.starts_with("0b") || text.starts_with("0B") => 2,
        _ => 10,
    }
}

fn parse_long_literal(spelling: &str) -> Option<i64> {
    let digits = spelling.strip_suffix(['l', 'L'])?.replace('_', "");
    let radix = integer_radix(Some(&digits));
    let digits = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
        .or_else(|| digits.strip_prefix("0b"))
        .or_else(|| digits.strip_prefix("0B"))
        .unwrap_or(&digits);
    i64::from_str_radix(digits, radix).ok()
}

fn parse_float_literal(spelling: &str) -> Option<f32> {
    spelling
        .strip_suffix(['f', 'F'])?
        .replace('_', "")
        .parse()
        .ok()
}

fn parse_double_literal(spelling: &str) -> Option<f64> {
    spelling
        .strip_suffix(['d', 'D'])?
        .replace('_', "")
        .parse()
        .ok()
}

enum DecodedString {
    Scalar(String),
    Utf16(Vec<u16>),
}

fn decode_string_literal(text: &str) -> Option<DecodedString> {
    let multiline = text.starts_with("\"\"\"");
    let body = if multiline {
        text.strip_prefix("\"\"\"")?.strip_suffix("\"\"\"")?
    } else {
        text.strip_prefix('"')?.strip_suffix('"')?
    };
    let characters: Vec<char> = body.chars().collect();
    let mut units = Vec::new();
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];
        index += 1;
        if multiline || character != '\\' {
            let mut encoded = [0; 2];
            units.extend(character.encode_utf16(&mut encoded).iter().copied());
            continue;
        }

        let escaped = *characters.get(index)?;
        index += 1;
        match escaped {
            'b' => units.push('\u{0008}' as u16),
            't' => units.push('\t' as u16),
            'n' => units.push('\n' as u16),
            'f' => units.push('\u{000c}' as u16),
            'r' => units.push('\r' as u16),
            '\\' => units.push('\\' as u16),
            '"' => units.push('"' as u16),
            '\'' => units.push('\'' as u16),
            'u' | 'U' => {
                while characters
                    .get(index)
                    .is_some_and(|character| matches!(character, 'u' | 'U'))
                {
                    index += 1;
                }
                let digits: String = characters.get(index..index + 4)?.iter().collect();
                let code_unit = u16::from_str_radix(&digits, 16).ok()?;
                units.push(code_unit);
                index += 4;
            }
            '0'..='7' => {
                let mut digits = String::from(escaped);
                while digits.len() < 3
                    && characters
                        .get(index)
                        .is_some_and(|character| ('0'..='7').contains(character))
                {
                    digits.push(characters[index]);
                    index += 1;
                }
                let code_unit = u16::from_str_radix(&digits, 8).ok()?;
                units.push(code_unit);
            }
            _ => return None,
        }
    }

    Some(match String::from_utf16(&units) {
        Ok(value) => DecodedString::Scalar(value),
        Err(_) => DecodedString::Utf16(units),
    })
}
