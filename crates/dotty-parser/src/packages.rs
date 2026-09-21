use dotty_core::ast::{Ident, PackageDef, Select};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::statements::{ParsedStatement, StatementSequenceBoundary};
use crate::{Location, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_package_definition(&mut self, _location: Location) -> ParsedStatement {
        let mark = self.mark();
        self.advance();

        if self.current().kind == TokenKind::Keyword(HardKeyword::Object) {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "package objects are not supported yet",
            );
            self.advance();
            self.recover_until(crate::RecoverySet::Statement);
            return ParsedStatement::Expression(self.error_expr(self.current_span()));
        }

        let name = self.parse_package_name();
        let stats = self.parse_package_body();
        ParsedStatement::Definition(
            self.alloc_from(mark, TreeKind::PackageDef(PackageDef { name, stats })),
        )
    }

    fn parse_package_name(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        if !self.is_package_name() {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected a package name",
            );
            if self.current().kind != TokenKind::Eof {
                self.advance();
            }
            return self.error_expr(position);
        }

        let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
        let Ok(name) = self.intern_current_term_name() else {
            return self.error_expr(self.current_span());
        };
        self.advance();
        let mut tree = self.alloc_from(
            mark,
            TreeKind::Ident(Ident {
                name: *name.as_name(),
                backquoted,
            }),
        );

        while self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
            if !self.is_package_name() {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a package name after `.`",
                );
                break;
            }
            let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
            let Ok(name) = self.intern_current_term_name() else {
                break;
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

    fn parse_package_body(&mut self) -> Vec<TreeId<Untyped>> {
        if self.current().kind == TokenKind::Eof {
            return Vec::new();
        }

        let end = if self.accept(TokenKind::Punctuation(Punctuation::LeftBrace)) {
            Some(TokenKind::Punctuation(Punctuation::RightBrace))
        } else if self.accept_package_layout_start() {
            Some(TokenKind::Outdent)
        } else {
            None
        };

        let Some(end) = end else {
            self.consume_package_separators();
            return self.parse_unbraced_package_body();
        };

        let stats = self.with_location(Location::InBlock, |parser| {
            parser.with_block_end(Some(end), |parser| {
                parser.parse_top_level_sequence(StatementSequenceBoundary::Block(end))
            })
        });

        if end == TokenKind::Outdent && self.current().kind != TokenKind::Outdent {
            self.observe_outdented();
        }
        if !self.accept(end) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                format!("expected {end:?} to close package body"),
            );
        }
        stats
    }

    fn parse_unbraced_package_body(&mut self) -> Vec<TreeId<Untyped>> {
        self.parse_top_level_sequence(StatementSequenceBoundary::CompilationUnit)
    }

    fn accept_package_layout_start(&mut self) -> bool {
        if matches!(
            self.current().kind,
            TokenKind::ColonFollow | TokenKind::ColonOp | TokenKind::ColonEol
        ) {
            if self.current().kind != TokenKind::ColonEol {
                self.observe_colon_eol(false);
            }
            if self.current().kind == TokenKind::ColonEol {
                self.observe_indented();
                self.advance();
            }
        }
        self.accept(TokenKind::Indent)
    }

    fn consume_package_separators(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Punctuation(Punctuation::Semicolon)
        ) {
            self.advance();
        }
    }

    fn is_package_name(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, TextRange};

    #[test]
    fn parses_an_empty_package_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "package foo",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_package_definition(Location::Elsewhere)
        else {
            panic!("expected package definition");
        };
        let TreeKind::PackageDef(package) = &parser.ast().get(id).kind else {
            panic!("expected package tree");
        };
        assert!(package.stats.is_empty());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 11).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_qualified_braced_package_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "package foo.bar { import baz.qux }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::Dot), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 16, 17),
                token(TokenKind::Keyword(HardKeyword::Import), 18, 24),
                token(TokenKind::Identifier, 25, 28),
                token(TokenKind::Punctuation(Punctuation::Dot), 28, 29),
                token(TokenKind::Identifier, 29, 32),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 33, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_package_definition(Location::Elsewhere)
        else {
            panic!("expected package definition");
        };
        let TreeKind::PackageDef(package) = &parser.ast().get(id).kind else {
            panic!("expected package tree");
        };
        assert_eq!(package.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(package.stats[0]).kind,
            TreeKind::Import(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }
}
