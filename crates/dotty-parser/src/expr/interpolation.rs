use dotty_core::ast::{Block, InterpolatedString, Literal, This, UntypedNode};
use dotty_core::{Constant, Punctuation, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use crate::{ExpressionIssue, Location, ParseIssue, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_interpolated_string(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.parse_interpolated_string_with_mode(mark, false)
    }

    pub(crate) fn parse_interpolated_string_pattern(
        &mut self,
        mark: crate::Mark,
    ) -> TreeId<Untyped> {
        self.parse_interpolated_string_with_mode(mark, true)
    }

    fn parse_interpolated_string_with_mode(
        &mut self,
        mark: crate::Mark,
        in_pattern: bool,
    ) -> TreeId<Untyped> {
        let prefix = match self.intern_current_term_name() {
            Ok(prefix) => *prefix.as_name(),
            Err(_) => return self.unexpected_expression(),
        };
        self.advance();

        if self.current().kind != TokenKind::StringPart {
            self.report_issue(ParseIssue::Expression(
                ExpressionIssue::ExpectedInterpolatedStringPart {
                    found: self.current().kind,
                },
            ));
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(InterpolatedString {
                    prefix,
                    parts: Vec::new(),
                })),
            );
        }

        let delimiter = if self
            .current_text()
            .unwrap_or_default()
            .starts_with("\"\"\"")
        {
            "\"\"\""
        } else {
            "\""
        };
        let mut parts = Vec::new();
        while self.current().kind == TokenKind::StringPart {
            // The scanner includes the splice dollar or the closing quotes
            // in this token's span. The following token belongs to the
            // enclosing grammar once the closing delimiter has been read.
            let text = self.current_text().unwrap_or_default();
            let has_splice = text.ends_with('$');
            let start = self.current().span.start();
            let content = if parts.is_empty() {
                text.strip_prefix(delimiter).unwrap_or(text)
            } else {
                text
            };
            let content_start = start + (text.len() - content.len()) as u32;
            let content = content
                .strip_suffix(if has_splice { "$" } else { delimiter })
                .unwrap_or(content);
            parts.push(self.parse_interpolated_part(content, content_start));
            if !has_splice {
                break;
            }

            match self.current().kind {
                TokenKind::Identifier => {
                    if self.current_text_is("this") {
                        let position = self.current_span();
                        self.advance();
                        parts.push(self.alloc(TreeKind::This(This { qual: None }), Some(position)));
                    } else if in_pattern {
                        parts.push(self.with_parse_kind(ParseKind::Pattern, |parser| {
                            parser.with_location(Location::InPattern, |parser| parser.pattern())
                        }));
                    } else {
                        parts.push(self.simple_expr());
                    }
                }
                TokenKind::Punctuation(Punctuation::LeftBrace) => {
                    if in_pattern {
                        let block_mark = self.mark();
                        self.advance();
                        let pattern = self.with_parse_kind(ParseKind::Pattern, |parser| {
                            parser.with_location(Location::InPattern, |parser| parser.pattern())
                        });
                        if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
                            self.report_issue(ParseIssue::Expression(
                                ExpressionIssue::ExpectedInterpolatedPatternSpliceCloseBrace {
                                    found: self.current().kind,
                                },
                            ));
                        }
                        parts.push(self.alloc_from(
                            block_mark,
                            TreeKind::Block(Block {
                                stats: Vec::new(),
                                expr: pattern,
                            }),
                        ));
                    } else {
                        parts.push(self.expr());
                    }
                }
                _ => break,
            }
        }

        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(InterpolatedString {
                prefix,
                parts,
            })),
        )
    }

    fn parse_interpolated_part(&mut self, source: &str, start: u32) -> TreeId<Untyped> {
        let value = interpolated_fragment_value(source);
        let length = if value.len() == source.len() {
            source.len()
        } else {
            // Dotty measures decoded length in UTF-16 from this fragment's start.
            let mut remaining = value.encode_utf16().count();
            let mut bytes = 0;
            for ch in source.chars() {
                if remaining == 0 {
                    break;
                }
                bytes += ch.len_utf8();
                remaining = remaining.saturating_sub(ch.len_utf16());
            }
            bytes
        };
        let end = start + length as u32;
        let range = TextRange::new(start, end).expect("interpolation span is ordered");
        self.advance();
        let value = self.names.intern(&value);
        self.alloc(
            TreeKind::Literal(Literal {
                value: Constant::String(value),
            }),
            Some(dotty_core::SourceSpan::new(
                self.source_id,
                Span::without_point(range),
            )),
        )
    }
}

fn interpolated_fragment_value(source: &str) -> String {
    let mut value = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '$'
            && let Some(&escaped @ ('$' | '"')) = characters.peek()
        {
            characters.next();
            value.push(escaped);
        } else {
            value.push(character);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Ident, InterpolatedString, Literal};
    use dotty_core::{Constant, NameInterner, Punctuation, TextRange};

    #[test]
    fn parses_simple_interpolation_parts() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s\"hello $name!\"",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 9),
                token(TokenKind::Identifier, 9, 13),
                token(TokenKind::StringPart, 13, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            parser.ast().get(tree).kind.clone()
        else {
            panic!("expected interpolated string");
        };

        assert_eq!(parser.names.resolve(interpolation.prefix.text()), "s");
        assert_eq!(interpolation.parts.len(), 3);
        assert_eq!(
            parser
                .ast()
                .get(interpolation.parts[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(2, 8).unwrap()
        );
        assert!(matches!(
            parser.ast().get(interpolation.parts[1]).kind,
            TreeKind::Ident(Ident { .. })
        ));
        assert_eq!(
            parser
                .ast()
                .get(interpolation.parts[2])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(13, 14).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_braced_interpolation_as_a_block_part() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s\"${foo}\"",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 3, 4),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 7, 8),
                token(TokenKind::StringPart, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(InterpolatedString {
            parts,
            ..
        })) = parser.ast().get(tree).kind.clone()
        else {
            panic!("expected interpolated string");
        };

        assert_eq!(parts.len(), 3);
        assert_eq!(
            parser.ast().get(parts[0]).position.unwrap().span().range(),
            TextRange::new(2, 2).unwrap()
        );
        assert!(matches!(
            parser.ast().get(parts[1]).kind,
            TreeKind::Block(_)
        ));
        assert_eq!(
            parser.ast().get(parts[2]).position.unwrap().span().range(),
            TextRange::new(8, 8).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn expression_interpolation_keeps_underscore_as_a_placeholder() {
        let source = "s\"${_}\"";
        let mut names = NameInterner::new();
        let parser = parser_for(
            source,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 3),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 5, 6),
                token(TokenKind::StringPart, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            crate::ParseDiagnosticKind::UnboundPlaceholderParameter
        );
    }

    #[test]
    fn reports_a_missing_initial_string_part_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();

        assert!(matches!(
            parser.ast().get(tree).kind,
            TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            crate::ParseDiagnosticKind::ExpectedExpression
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn escaped_quote_after_a_supplementary_character_has_a_fragment_local_span() {
        let mut names = NameInterner::new();
        let source = r#"s"😀$"x""#;
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );
        let root = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            &parser.ast().get(root).kind
        else {
            panic!("expected interpolation")
        };
        let [part] = interpolation.parts.as_slice() else {
            panic!("expected one fragment")
        };
        let literal = parser.ast().get(*part);
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = literal.kind
        else {
            panic!("expected string literal");
        };
        assert_eq!(parser.names.resolve(value), "😀\"x");
        assert_eq!(
            literal.position.unwrap().span().range(),
            TextRange::new(2, 8).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn uses_decoded_length_for_escaped_dollar_spans() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s\"cost $$\"",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            parser.ast().get(tree).kind.clone()
        else {
            panic!("expected interpolated string");
        };

        assert_eq!(interpolation.parts.len(), 1);
        assert_eq!(
            parser
                .ast()
                .get(interpolation.parts[0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(2, 8).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn stores_the_scanner_value_for_escaped_dollars() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s\"cost $$5\"",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            parser.ast().get(tree).kind.clone()
        else {
            panic!("expected interpolated string");
        };
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = &parser.ast().get(interpolation.parts[0]).kind
        else {
            panic!("expected string fragment literal");
        };

        assert_eq!(parser.names.resolve(*value), "cost $5");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_this_as_the_special_simple_splice() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s\"$this\"",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 3),
                token(TokenKind::Identifier, 3, 7),
                token(TokenKind::StringPart, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let tree = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(interpolation)) =
            parser.ast().get(tree).kind.clone()
        else {
            panic!("expected interpolated string");
        };

        assert_eq!(interpolation.parts.len(), 3);
        assert!(matches!(
            parser.ast().get(interpolation.parts[1]).kind,
            TreeKind::This(This { qual: None })
        ));
        assert_eq!(
            parser
                .ast()
                .get(interpolation.parts[1])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(3, 7).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }
}
