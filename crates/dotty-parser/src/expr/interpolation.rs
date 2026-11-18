use std::borrow::Cow;

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

        let mut parts = Vec::new();
        let delimiter = if self.current_text().is_ok_and(|s| s.starts_with("\"\"\"")) {
            "\"\"\""
        } else {
            "\""
        };
        let mut first_part = true;
        loop {
            if self.current().kind == TokenKind::StringPart {
                let (part, closed) = self.parse_interpolated_part(first_part, delimiter);
                parts.push(part);
                if closed {
                    break;
                }
                first_part = false;
            }

            match self.current().kind {
                TokenKind::Keyword(dotty_core::HardKeyword::This) => {
                    let position = self.current_span();
                    self.advance();
                    parts.push(self.alloc(TreeKind::This(This { qual: None }), Some(position)));
                }
                TokenKind::Identifier if in_pattern || !self.current_text_is("_") => {
                    if in_pattern {
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
                found @ (TokenKind::Keyword(_) | TokenKind::Identifier) => {
                    self.report_issue(ParseIssue::Expression(
                        ExpressionIssue::InvalidSimpleInterpolationSplice { found },
                    ));
                    self.advance();
                }
                TokenKind::StringPart => continue,
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

    fn parse_interpolated_part(&mut self, first: bool, delimiter: &str) -> (TreeId<Untyped>, bool) {
        let spelling = self.current_text().unwrap_or_default();
        let body = if first {
            spelling.strip_prefix(delimiter).unwrap_or(spelling)
        } else {
            spelling
        };
        let start = self.current().span.start() + (spelling.len() - body.len()) as u32;
        let (body, closed) = match body.strip_suffix(delimiter) {
            Some(final_part) => (final_part, true),
            None => (body.strip_suffix('$').unwrap_or(body), false),
        };
        let value = interpolated_fragment_value(body);
        // Dotty measures the source span using the decoded fragment's UTF-16 length.
        // A logical end inside a surrogate pair rounds up to a Rust character boundary.
        let byte_len = if value.len() == body.len() {
            body.len()
        } else {
            let mut remaining = value.encode_utf16().count();
            body.chars()
                .take_while(|ch| {
                    if remaining == 0 {
                        return false;
                    }
                    remaining = remaining.saturating_sub(ch.len_utf16());
                    true
                })
                .map(char::len_utf8)
                .sum()
        };
        let range = TextRange::new(start, start + byte_len as u32)
            .expect("fragment lies inside its source token");
        self.advance();
        let value = self.names.intern(&value);
        let part = self.alloc(
            TreeKind::Literal(Literal {
                value: Constant::String(value),
            }),
            Some(dotty_core::SourceSpan::new(
                self.source_id,
                Span::without_point(range),
            )),
        );
        (part, closed)
    }
}

fn interpolated_fragment_value(source: &str) -> Cow<'_, str> {
    if !source.contains('$') {
        return Cow::Borrowed(source);
    }
    let mut value = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$'
            && let Some(escaped) = chars.next_if(|ch| matches!(ch, '$' | '"'))
        {
            value.push(escaped);
        } else {
            value.push(ch);
        }
    }
    Cow::Owned(value)
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
    fn decodes_dollar_quote_and_maps_the_logical_span_within_the_fragment() {
        let source = r#"s"🦋$"🎁""#;
        let end = source.len() as u32;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, end),
                token(TokenKind::Eof, end, end),
            ],
            &mut names,
        );
        let tree = parser.simple_expr();
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(ref interpolation)) =
            parser.ast().get(tree).kind
        else {
            panic!("expected interpolation")
        };
        let literal = parser.ast().get(interpolation.parts[0]);
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = literal.kind
        else {
            panic!("expected literal")
        };
        assert_eq!(parser.names.resolve(value), "🦋\"🎁");
        // The logical UTF-16 end falls between the surrogates of the last emoji.
        assert_eq!(
            literal.position.unwrap().span().range(),
            TextRange::new(2, end - 1).unwrap()
        );
    }

    #[test]
    fn rejects_a_keyword_splice_at_its_span_and_keeps_the_next_splice() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            r#"s"$if$x""#,
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 3),
                token(TokenKind::Keyword(dotty_core::HardKeyword::If), 3, 5),
                token(TokenKind::StringPart, 5, 6),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::StringPart, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );
        let root = parser.simple_expr();
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].span(),
            TextRange::new(3, 5).unwrap()
        );
        assert_eq!(
            parser.diagnostics()[0].issue().code(),
            "parser.expression.invalid_simple_interpolation_splice"
        );
        let TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(ref tree)) =
            parser.ast().get(root).kind
        else {
            panic!("expected interpolation")
        };
        assert!(tree.parts.iter().any(|part| matches!(&parser.ast().get(*part).kind, TreeKind::Ident(id) if parser.names.resolve(id.name.text()) == "x")));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_this_as_the_special_simple_splice() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "s\"$this\"",
            vec![
                token(TokenKind::InterpolationId, 0, 1),
                token(TokenKind::StringPart, 1, 2),
                token(TokenKind::Keyword(dotty_core::HardKeyword::This), 3, 7),
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
