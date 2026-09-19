use dotty_core::ast::{Ident, Select};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the small simple-type subset needed by simple expressions.
    pub(crate) fn simple_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut tree = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_type_name() else {
                    return self.error_type(self.current_span());
                };
                self.advance();
                self.alloc_from(
                    mark,
                    TreeKind::Ident(Ident {
                        name: *name.as_name(),
                        backquoted,
                    }),
                )
            }
            _ => {
                let position = self.current_span();
                self.report(
                    crate::ParseDiagnosticKind::ExpectedType,
                    "expected a simple type",
                );
                if self.current().kind != TokenKind::Eof {
                    self.advance();
                }
                return self.error_type(position);
            }
        };

        while self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
            let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
            let name = match self.current().kind {
                TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                    match self.intern_current_type_name() {
                        Ok(name) => name,
                        Err(_) => return self.error_type(self.current_span()),
                    }
                }
                _ => {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedType,
                        "expected a type name after `.`",
                    );
                    return tree;
                }
            };
            self.advance();
            tree = self.alloc_from(
                mark,
                TreeKind::Select(Select {
                    qualifier: tree,
                    name: *name.as_name(),
                    backquoted,
                }),
            );
        }

        tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, Punctuation, TextRange};

    #[test]
    fn parses_a_simple_type_in_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Value",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Ident(ident) = parser.ast().get(id).kind else {
            panic!("expected type identifier");
        };

        assert!(ident.name.is_type());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 5).unwrap()
        );
        let name = ident.name;
        drop(parser);
        assert_eq!(names.resolve(name.text()), "Value");
    }

    #[test]
    fn parses_a_qualified_simple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "pkg.Value",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected qualified type");
        };

        assert!(selection.name.is_type());
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }
}
