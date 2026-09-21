//! Shared parsing infrastructure for class, trait, and object template bodies.
//!
//! A template body is a source-ordered list of members.  Unlike an expression
//! block, it must retain every entry, including expressions, because later
//! definition parsing and semantic phases decide how those entries are
//! interpreted.  The actual class-like definition parser is layered on top of
//! this helper.

use dotty_core::{Punctuation, TokenKind, TreeId, Untyped};

use crate::{Location, ParseDiagnosticKind, Parser, RecoverySet};

/// Delimiters accepted by the template-body parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateBody {
    Braced,
    Indented,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a template body and preserves all statements in source order.
    ///
    /// The opening and closing delimiter belong to this helper.  In the
    /// indented form the scanner has already classified the layout and the
    /// parser only consumes the resulting `Indent`/`Outdent` tokens.
    pub(crate) fn parse_template_body(&mut self, body: TemplateBody) -> Vec<TreeId<Untyped>> {
        let (opening, closing) = match body {
            TemplateBody::Braced => (
                TokenKind::Punctuation(Punctuation::LeftBrace),
                TokenKind::Punctuation(Punctuation::RightBrace),
            ),
            TemplateBody::Indented => (TokenKind::Indent, TokenKind::Outdent),
        };

        if !self.expect(opening) {
            return Vec::new();
        }

        let members = self.with_placeholder_scope(|parser| {
            parser.with_location(Location::InBlock, |parser| {
                parser.with_block_end(Some(closing), |parser| {
                    parser.parse_template_members(closing)
                })
            })
        });

        if !self.accept(closing) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                format!("expected {closing:?} to close template body"),
            );
        }

        members
    }

    fn parse_template_members(&mut self, closing: TokenKind) -> Vec<TreeId<Untyped>> {
        let mut members = Vec::new();
        self.consume_template_separators(closing);

        while !self.template_body_ended(closing) {
            let checkpoint = self.cursor.checkpoint();
            let statement = self.parse_statement(Location::InBlock);
            members.push(match statement {
                crate::statements::ParsedStatement::Definition(tree)
                | crate::statements::ParsedStatement::Expression(tree) => tree,
            });

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a template body",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if self.is_template_separator(self.current().kind) {
                self.consume_template_separators(closing);
            } else if !self.template_body_ended(closing) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "expected a template member separator",
                );
                self.recover_until(RecoverySet::Statement);
                self.consume_template_separators(closing);
            }
        }

        members
    }

    fn template_body_ended(&self, closing: TokenKind) -> bool {
        self.current().kind == closing || self.current().kind == TokenKind::Eof
    }

    fn is_template_separator(&self, kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Punctuation(Punctuation::Semicolon)
        )
    }

    fn consume_template_separators(&mut self, closing: TokenKind) {
        while self.is_template_separator(self.current().kind) && !self.template_body_ended(closing)
        {
            self.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{HardKeyword, NameInterner, TreeKind};

    #[test]
    fn braced_template_body_preserves_definitions_and_expressions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{ val x = 1\nfoo }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Keyword(HardKeyword::Val), 2, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 9),
                token(TokenKind::IntegerLiteral, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Braced);

        assert_eq!(members.len(), 2);
        assert!(matches!(
            parser.ast().get(members[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(
            parser.ast().get(members[1]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn indented_template_body_consumes_layout_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "  first\n  second",
            vec![
                token(TokenKind::Indent, 0, 0),
                token(TokenKind::Identifier, 2, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Outdent, 16, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Indented);

        assert_eq!(members.len(), 2);
        assert!(
            members
                .iter()
                .all(|member| matches!(parser.ast().get(*member).kind, TreeKind::Ident(_)))
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn empty_template_body_has_no_synthetic_expression_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "{}",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let members = parser.parse_template_body(TemplateBody::Braced);

        assert!(members.is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }
}
