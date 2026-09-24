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

        self.parse_number_with_spelling(mark, token_kind, spelling)
    }

    /// Parses a numeric literal as a semantic constant for type syntax.
    /// Expression parsing deliberately keeps unsuffixed numbers as raw
    /// `Number` nodes, but Dotty's `SingletonTypeTree` wraps a `Literal`.
    pub(crate) fn parse_number_constant(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let token_kind = self.current().kind;
        let spelling = match self.current_text() {
            Ok(spelling) => spelling,
            Err(_) => return self.unexpected_expression(),
        };
        let Some(value) = numeric_constant(token_kind, spelling) else {
            return self.unexpected_expression();
        };
        self.advance();
        self.alloc_from(mark, TreeKind::Literal(Literal { value }))
    }

    pub(crate) fn parse_negative_number(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let token_kind = self.current().kind;
        let token_spelling = match self.current_text() {
            Ok(spelling) => spelling,
            Err(_) => return self.unexpected_expression(),
        };
        let spelling = format!("-{token_spelling}");
        self.parse_number_with_spelling(mark, token_kind, &spelling)
    }

    fn parse_number_with_spelling(
        &mut self,
        mark: crate::Mark,
        token_kind: TokenKind,
        spelling: &str,
    ) -> TreeId<Untyped> {
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
                        value: Constant::float(value),
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
                        value: Constant::double(value),
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
    match text.map(|text| {
        text.strip_prefix('-')
            .or_else(|| text.strip_prefix('+'))
            .unwrap_or(text)
    }) {
        Some(text) if text.starts_with("0x") || text.starts_with("0X") => 16,
        Some(text) if text.starts_with("0b") || text.starts_with("0B") => 2,
        _ => 10,
    }
}

fn parse_long_literal(spelling: &str) -> Option<i64> {
    let digits = spelling.strip_suffix(['l', 'L'])?.replace('_', "");
    let (negative, digits) = if let Some(digits) = digits.strip_prefix('-') {
        (true, digits)
    } else if let Some(digits) = digits.strip_prefix('+') {
        (false, digits)
    } else {
        (false, digits.as_str())
    };
    let radix = integer_radix(Some(digits));
    let digits = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
        .or_else(|| digits.strip_prefix("0b"))
        .or_else(|| digits.strip_prefix("0B"))
        .unwrap_or(digits);
    let value = u64::from_str_radix(digits, radix).ok()?;
    if negative {
        if value == 1_u64 << 63 {
            Some(i64::MIN)
        } else {
            i64::try_from(value).ok()?.checked_neg()
        }
    } else {
        i64::try_from(value).ok()
    }
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

fn numeric_constant(token_kind: TokenKind, spelling: &str) -> Option<Constant> {
    match token_kind {
        TokenKind::IntegerLiteral => parse_integer_literal(spelling).map(Constant::Int),
        TokenKind::LongLiteral => parse_long_literal(spelling).map(Constant::Long),
        TokenKind::DecimalLiteral | TokenKind::ExponentLiteral => {
            spelling.replace('_', "").parse().ok().map(Constant::double)
        }
        TokenKind::FloatLiteral => parse_float_literal(spelling).map(Constant::float),
        TokenKind::DoubleLiteral => parse_double_literal(spelling).map(Constant::double),
        _ => None,
    }
}

fn parse_integer_literal(spelling: &str) -> Option<i32> {
    let digits = spelling.replace('_', "");
    let radix = integer_radix(Some(&digits));
    let digits = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
        .or_else(|| digits.strip_prefix("0b"))
        .or_else(|| digits.strip_prefix("0B"))
        .unwrap_or(&digits);
    let value = u64::from_str_radix(digits, radix).ok()?;
    i32::try_from(value).ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Literal, UntypedNode};
    use dotty_core::{HardKeyword, NameInterner};

    #[test]
    fn parses_an_integer_as_a_raw_whole_number() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "42",
            vec![
                token(TokenKind::IntegerLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = tree.kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(10));
        assert_eq!(names.resolve(number.text), "42");
    }

    #[test]
    fn preserves_hexadecimal_integer_radix() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "0xff",
            vec![
                token(TokenKind::IntegerLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = parser.ast().get(id).kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(16));
    }

    #[test]
    fn preserves_binary_integer_radix() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "0b1010",
            vec![
                token(TokenKind::IntegerLiteral, 0, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = parser.ast().get(id).kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(2));
    }

    #[test]
    fn decodes_a_decimal_long_literal_as_a_long_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1L",
            vec![
                token(TokenKind::LongLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Long(1)
            })
        ));
    }

    #[test]
    fn decodes_a_hexadecimal_long_literal_as_a_long_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "0xffL",
            vec![
                token(TokenKind::LongLiteral, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Long(255)
            })
        ));
    }

    #[test]
    fn decodes_a_float_suffix_as_a_float_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1.5f",
            vec![
                token(TokenKind::FloatLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                ref value
            }) if *value == Constant::FloatBits(0x3fc0_0000)
        ));
    }

    #[test]
    fn decodes_a_double_suffix_as_a_double_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1.5d",
            vec![
                token(TokenKind::DoubleLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                ref value
            }) if *value == Constant::DoubleBits(0x3ff8_0000_0000_0000)
        ));
    }

    #[test]
    fn parses_a_string_literal_without_retaining_quote_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"hello\"",
            vec![
                token(TokenKind::StringLiteral, 0, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = tree.kind
        else {
            panic!("expected string literal tree");
        };
        assert_eq!(names.resolve(value), "hello");
    }

    #[test]
    fn decodes_escaped_characters_in_a_string_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"a\\n\"",
            vec![
                token(TokenKind::StringLiteral, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "a\n");
    }

    #[test]
    fn decodes_an_uppercase_unicode_escape_in_a_string_literal() {
        let source = r#""\U0041""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "A");
    }

    #[test]
    fn decodes_repeated_uppercase_unicode_escape_prefixes() {
        let source = r#""\UU0041""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "A");
    }

    #[test]
    fn decodes_mixed_case_unicode_escape_prefixes() {
        let source = r#""\uU0041""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "A");
    }

    #[test]
    fn decodes_a_surrogate_pair_to_a_scalar_string_value() {
        let source = r#""\uD834\uDD1E""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected scalar string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "𝄞");
    }

    #[test]
    fn preserves_an_unpaired_surrogate_as_utf16_code_units() {
        let source = r#""a\uD800b""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Literal(Literal {
            value: Constant::StringUtf16(units),
        }) = tree.kind
        else {
            panic!("expected UTF-16 string literal tree");
        };
        assert_eq!(units, vec!['a' as u16, 0xD800, 'b' as u16]);
    }

    #[test]
    fn preserves_newlines_in_a_triple_quoted_string_literal() {
        let source = "\"\"\"a\nb\"\"\"";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.simple_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "a\nb");
    }

    #[test]
    fn parses_true_as_a_boolean_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "true",
            vec![
                token(TokenKind::Keyword(HardKeyword::True), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Boolean(true)
            })
        ));
    }

    #[test]
    fn parses_false_as_a_boolean_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "false",
            vec![
                token(TokenKind::Keyword(HardKeyword::False), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Boolean(false)
            })
        ));
    }

    #[test]
    fn parses_null_as_a_null_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "null",
            vec![
                token(TokenKind::Keyword(HardKeyword::Null), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Null
            })
        ));
    }
}
