use dotty_core::ast::{Export, Ident, Import, ImportSelector, Select};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_import_clause(&mut self, _location: Location) -> Vec<TreeId<Untyped>> {
        self.parse_import_or_export_clause(false)
    }

    pub(crate) fn parse_export_clause(&mut self, _location: Location) -> Vec<TreeId<Untyped>> {
        self.parse_import_or_export_clause(true)
    }

    fn parse_import_or_export_clause(&mut self, is_export: bool) -> Vec<TreeId<Untyped>> {
        let keyword_mark = self.mark();
        self.advance();
        let mut trees = Vec::new();

        loop {
            let mark = if trees.is_empty() {
                keyword_mark
            } else {
                self.mark()
            };
            let (expr, selectors) = self.parse_import_expr();
            let tree = if is_export {
                self.alloc_from(mark, TreeKind::Export(Export { expr, selectors }))
            } else {
                self.alloc_from(mark, TreeKind::Import(Import { expr, selectors }))
            };
            trees.push(tree);
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }

        trees
    }

    /// Parses an import/export expression while keeping the qualifier and its
    /// selectors separate, as in Dotty's source-level AST.
    fn parse_import_expr(&mut self) -> (TreeId<Untyped>, Vec<ImportSelector<Untyped>>) {
        let path_mark = self.mark();
        let mut qualifier = self.parse_import_name(path_mark);

        if self.current_is_as() {
            let imported = match self.ast.get(qualifier).kind {
                TreeKind::Ident(ident) => ident.name,
                _ => {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected an importable name before `as`",
                    );
                    return (qualifier, Vec::new());
                }
            };
            let selector = self.parse_named_selector(imported);
            let empty_name_id = self.names.intern("<empty>");
            let empty_expr = self.alloc(
                TreeKind::Ident(Ident {
                    name: *dotty_core::TermName::new(empty_name_id).as_name(),
                    backquoted: false,
                }),
                Some(self.zero_width_span(path_mark.start())),
            );
            return (empty_expr, vec![selector]);
        }

        loop {
            if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `.` and an imported name",
                );
                return (qualifier, Vec::new());
            }

            if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBrace) {
                return (qualifier, self.parse_braced_selectors());
            }

            if self.current_is_import_wildcard() || self.current_is_legacy_wildcard() {
                return (qualifier, vec![self.parse_wildcard_selector()]);
            }

            if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Given) {
                return (qualifier, vec![self.parse_given_selector()]);
            }

            if !self.is_import_name() {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an imported name after `.`",
                );
                return (qualifier, Vec::new());
            }

            let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
            let Ok(name) = self.intern_current_term_name() else {
                return (qualifier, Vec::new());
            };
            self.advance();

            if self.current().kind == TokenKind::Punctuation(Punctuation::Dot) {
                qualifier = self.alloc_from(
                    path_mark,
                    TreeKind::Select(Select {
                        qualifier,
                        name: *name.as_name(),
                        backquoted,
                    }),
                );
                continue;
            }

            return (qualifier, vec![self.parse_named_selector(*name.as_name())]);
        }
    }

    fn parse_braced_selectors(&mut self) -> Vec<ImportSelector<Untyped>> {
        self.advance();
        let mut selectors = Vec::new();
        let mut names_allowed = true;

        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Eof
        ) {
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                continue;
            }

            let wildcard = self.current_is_import_wildcard()
                || self.current_is_legacy_wildcard()
                || self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Given);
            if !names_allowed && !wildcard {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "named import/export selectors cannot follow a wildcard or `given` selector",
                );
            }

            let selector =
                if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Given) {
                    self.parse_given_selector()
                } else if self.current_is_import_wildcard() || self.current_is_legacy_wildcard() {
                    self.parse_wildcard_selector()
                } else if self.is_import_name() {
                    let name = self.intern_current_term_name();
                    let Ok(name) = name else {
                        self.advance();
                        continue;
                    };
                    self.advance();
                    self.parse_named_selector(*name.as_name())
                } else {
                    let position = self.current_span();
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected an import/export selector",
                    );
                    if self.current().kind != TokenKind::Eof {
                        self.advance();
                    }
                    self.error_selector(position)
                };

            names_allowed &= !wildcard;
            selectors.push(selector);
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma))
                && !matches!(
                    self.current().kind,
                    TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Eof
                )
            {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `,` or `}` after an import/export selector",
                );
                self.recover_until(crate::RecoverySet::Statement);
                break;
            }
        }

        if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `}` after import/export selectors",
            );
        }
        selectors
    }

    fn parse_named_selector(&mut self, imported: dotty_core::Name) -> ImportSelector<Untyped> {
        let renamed = if self.current_is_as() {
            self.advance();
            if self.is_import_name() || self.current_is_legacy_wildcard() {
                let mark = self.mark();
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return ImportSelector {
                        imported,
                        renamed: None,
                        bound: None,
                    };
                };
                self.advance();
                Some(self.alloc_from(
                    mark,
                    TreeKind::Ident(Ident {
                        name: *name.as_name(),
                        backquoted,
                    }),
                ))
            } else {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an identifier after `as`",
                );
                None
            }
        } else {
            None
        };

        ImportSelector {
            imported,
            renamed,
            bound: None,
        }
    }

    fn parse_wildcard_selector(&mut self) -> ImportSelector<Untyped> {
        let legacy = self.current_is_legacy_wildcard();
        if legacy {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "`_` is no longer supported for a wildcard import/export; use `*` instead",
            );
        }
        let imported = self.names.intern("*");
        self.advance();
        ImportSelector {
            imported: dotty_core::Name::new(imported, dotty_core::Namespace::Term),
            renamed: None,
            bound: None,
        }
    }

    fn parse_given_selector(&mut self) -> ImportSelector<Untyped> {
        self.advance();
        let empty = self.names.intern("");
        let bound = if self.can_start_import_type() {
            Some(self.with_parse_kind(ParseKind::Type, |parser| parser.simple_type()))
        } else {
            None
        };
        ImportSelector {
            imported: dotty_core::Name::new(empty, dotty_core::Namespace::Term),
            renamed: None,
            bound,
        }
    }

    fn error_selector(&mut self, position: dotty_core::SourceSpan) -> ImportSelector<Untyped> {
        let empty = self.names.intern("");
        let bound = Some(self.error_expr(position));
        ImportSelector {
            imported: dotty_core::Name::new(empty, dotty_core::Namespace::Term),
            renamed: None,
            bound,
        }
    }

    fn parse_import_name(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        if !self.is_import_name() {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected an import or export qualifier",
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
        self.alloc_from(
            mark,
            TreeKind::Ident(Ident {
                name: *name.as_name(),
                backquoted,
            }),
        )
    }

    fn is_import_name(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        )
    }

    fn current_is_import_wildcard(&mut self) -> bool {
        self.current().kind == TokenKind::Operator && self.current_text_is("*")
    }

    fn current_is_legacy_wildcard(&mut self) -> bool {
        self.current().kind == TokenKind::Identifier && self.current_text_is("_")
    }

    fn current_is_as(&mut self) -> bool {
        self.current().kind == TokenKind::Identifier && self.current_text_is("as")
    }

    fn can_start_import_type(&self) -> bool {
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
    use dotty_core::{HardKeyword, NameInterner, TextRange, TreeKind};

    #[test]
    fn parses_a_qualified_import_as_a_qualifier_and_selector() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.bar",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        assert_eq!(import.selectors.len(), 1);
        let imported = import.selectors[0].imported;
        assert!(matches!(
            parser.ast().get(import.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(ids[0]).position.unwrap().span().range(),
            TextRange::new(0, 14).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "bar");
    }

    #[test]
    fn parses_a_direct_alias_import_without_a_qualifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo as bar",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Identifier, 11, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        assert_eq!(import.selectors.len(), 1);
        assert!(import.selectors[0].renamed.is_some());
        let empty_name = match parser.ast().get(import.expr).kind {
            TreeKind::Ident(ident) => ident.name,
            _ => panic!("expected a synthetic empty import expression"),
        };
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(empty_name.text()), "<empty>");
    }

    #[test]
    fn parses_a_wildcard_import_without_an_expression_node_for_the_star() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.*",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Operator, 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        let imported = import.selectors[0].imported;
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "*");
    }

    #[test]
    fn parses_export_with_the_same_source_shape_as_import() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "export foo.bar",
            vec![
                token(TokenKind::Keyword(HardKeyword::Export), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let ids = parser.parse_export_clause(Location::Elsewhere);
        assert!(matches!(parser.ast().get(ids[0]).kind, TreeKind::Export(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_braced_selectors_with_renames_and_hiding() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{bar as baz, qux as _}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Identifier, 16, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::Comma), 22, 23),
                token(TokenKind::Identifier, 24, 27),
                token(TokenKind::Identifier, 28, 30),
                token(TokenKind::Identifier, 31, 32),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 32, 33),
                token(TokenKind::Eof, 33, 33),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        assert_eq!(import.selectors.len(), 2);
        assert!(import.selectors[0].renamed.is_some());
        assert!(import.selectors[1].renamed.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_typed_given_selector() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.given Ordering",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Given), 11, 16),
                token(TokenKind::Identifier, 17, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        assert!(import.selectors[0].bound.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_a_named_selector_after_a_wildcard() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{*, bar}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Operator, 12, 13),
                token(TokenKind::Punctuation(Punctuation::Comma), 13, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        parser.parse_import_clause(Location::Elsewhere);

        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
    }

    #[test]
    fn parses_multiple_import_expressions_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.bar, baz.qux",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Punctuation(Punctuation::Comma), 14, 15),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::Punctuation(Punctuation::Dot), 19, 20),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);

        assert_eq!(ids.len(), 2);
        assert!(matches!(parser.ast().get(ids[0]).kind, TreeKind::Import(_)));
        assert!(matches!(parser.ast().get(ids[1]).kind, TreeKind::Import(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn import_statement_is_retained_as_a_block_stat() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.bar\nx",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Newline, 14, 15),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let (stats, expr) = parser.parse_statement_sequence(
            crate::statements::StatementSequenceBoundary::CompilationUnit,
        );

        assert_eq!(stats.len(), 1);
        assert!(matches!(
            parser.ast().get(stats[0]).kind,
            TreeKind::Import(_)
        ));
        assert!(matches!(parser.ast().get(expr).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }
}
