use dotty_core::ast::{Export, Ident, Import, ImportSelector, Select};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_import_clause(&mut self, _location: Location) -> TreeId<Untyped> {
        self.parse_import_or_export_clause(false)
    }

    pub(crate) fn parse_export_clause(&mut self, _location: Location) -> TreeId<Untyped> {
        self.parse_import_or_export_clause(true)
    }

    fn parse_import_or_export_clause(&mut self, is_export: bool) -> TreeId<Untyped> {
        let mark = self.mark();
        self.advance();

        let (expr, selectors) = self.parse_import_expr();
        if is_export {
            self.alloc_from(mark, TreeKind::Export(Export { expr, selectors }))
        } else {
            self.alloc_from(mark, TreeKind::Import(Import { expr, selectors }))
        }
    }

    /// Parses the first, deliberately small import/export expression subset:
    /// `qualifier.member` and `qualifier.*`. Selector lists and renames are
    /// layered on top of this shared path parser in later increments.
    fn parse_import_expr(&mut self) -> (TreeId<Untyped>, Vec<ImportSelector<Untyped>>) {
        let path_mark = self.mark();
        let mut qualifier = self.parse_import_name(path_mark);

        loop {
            if !self.accept(TokenKind::Punctuation(Punctuation::Dot)) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `.` and an imported name",
                );
                return (qualifier, Vec::new());
            }

            if self.current_is_import_wildcard() {
                let name = self.intern_current_term_name();
                let imported = match name {
                    Ok(name) => *name.as_name(),
                    Err(_) => {
                        return (qualifier, Vec::new());
                    }
                };
                self.advance();
                return (
                    qualifier,
                    vec![ImportSelector {
                        imported,
                        renamed: None,
                        bound: None,
                    }],
                );
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

            return (
                qualifier,
                vec![ImportSelector {
                    imported: *name.as_name(),
                    renamed: None,
                    bound: None,
                }],
            );
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

        let id = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(id).kind else {
            panic!("expected import tree");
        };
        assert_eq!(import.selectors.len(), 1);
        let imported = import.selectors[0].imported;
        assert!(matches!(
            parser.ast().get(import.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 14).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "bar");
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

        let id = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(id).kind else {
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

        let id = parser.parse_export_clause(Location::Elsewhere);
        assert!(matches!(parser.ast().get(id).kind, TreeKind::Export(_)));
        assert!(parser.diagnostics().is_empty());
    }
}
