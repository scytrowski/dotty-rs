use dotty_core::ast::{InterpolatedString, Literal, UntypedNode};
use dotty_core::{Constant, Punctuation, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn parse_interpolated_string(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let prefix = match self.intern_current_term_name() {
            Ok(prefix) => *prefix.as_name(),
            Err(_) => return self.unexpected_expression(),
        };
        self.advance();

        let mut parts = Vec::new();
        let mut first_part = true;
        loop {
            if self.current().kind == TokenKind::StringPart {
                parts.push(self.parse_interpolated_part(first_part));
                first_part = false;
            }

            match self.current().kind {
                TokenKind::Identifier => {
                    parts.push(self.simple_expr());
                }
                TokenKind::Punctuation(Punctuation::LeftBrace) => {
                    parts.push(self.expr());
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

    fn parse_interpolated_part(&mut self, first: bool) -> TreeId<Untyped> {
        let token = self.current().clone();
        let source = self.current_text().unwrap_or_default();
        let delimiter_len = if source.starts_with("\"\"\"") { 3 } else { 1 };
        let mut start = token.span.start();
        let mut end = token.span.end();

        if first {
            start = start.saturating_add(delimiter_len);
        }
        match self.cursor.lookahead(1).kind {
            TokenKind::Identifier | TokenKind::Punctuation(Punctuation::LeftBrace) => {
                end = end.saturating_sub(1);
            }
            TokenKind::StringPart => {}
            _ => end = end.saturating_sub(delimiter_len),
        }
        if end < start {
            end = start;
        }

        let value = self
            .source
            .slice(TextRange::new(start, end).expect("interpolation span is ordered"))
            .unwrap_or_default();
        let range = TextRange::new(start, end).expect("interpolation span is ordered");
        self.advance();
        let value = self.names.intern(value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{Ident, InterpolatedString};
    use dotty_core::{NameInterner, Punctuation, TextRange};

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
}
