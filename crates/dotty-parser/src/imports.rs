use dotty_core::ast::{Export, Ident, Import, ImportSelector, Select};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_import_clause(&mut self, _location: Location) -> Vec<TreeId<Untyped>> {
        let imports = self.parse_import_or_export_clause(false);
        for import_id in &imports {
            if self.is_capture_checking_language_import(*import_id) {
                // Dotty records this compilation-unit feature before checking
                // whether the import was written at the outermost level.
                self.context.features.capture_checking = true;
                if !self.outermost_imports_allowed {
                    let span = self
                        .ast
                        .get(*import_id)
                        .position
                        .unwrap_or_else(|| self.current_span());
                    self.report_at(
                        ParseDiagnosticKind::ExpectedToken,
                        span,
                        "this language import is only allowed at the toplevel",
                    );
                }
            }
        }
        imports
    }

    fn is_capture_checking_language_import(&self, import_id: TreeId<Untyped>) -> bool {
        let TreeKind::Import(import) = &self.ast.get(import_id).kind else {
            return false;
        };

        let language_import = self.matches_name_path(import.expr, &["language", "experimental"])
            || self.matches_name_path(import.expr, &["scala", "language", "experimental"])
            || self.matches_name_path(
                import.expr,
                &["_root_", "scala", "language", "experimental"],
            );
        language_import
            && import.selectors.iter().any(|selector| {
                self.names.resolve(selector.imported.text()) == "captureChecking"
                    && selector.renamed.is_none()
                    && selector.bound.is_none()
            })
    }

    fn matches_name_path(&self, tree_id: TreeId<Untyped>, expected: &[&str]) -> bool {
        let Some((last, prefix)) = expected.split_last() else {
            return false;
        };
        match &self.ast.get(tree_id).kind {
            TreeKind::Ident(identifier) => {
                prefix.is_empty() && self.names.resolve(identifier.name.text()) == *last
            }
            TreeKind::Select(selection) => {
                self.names.resolve(selection.name.text()) == *last
                    && self.matches_name_path(selection.qualifier, prefix)
            }
            _ => false,
        }
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
            let backquoted = match self.ast.get(qualifier).kind {
                TreeKind::Ident(ident) => ident.backquoted,
                _ => false,
            };
            let selector = self.parse_named_selector(imported, backquoted);
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

            let Some((name, backquoted)) = self.current_term_name() else {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an imported name after `.`",
                );
                return (qualifier, Vec::new());
            };
            self.advance();

            if self.current().kind == TokenKind::Punctuation(Punctuation::Dot) {
                qualifier = self.alloc_from(
                    path_mark,
                    TreeKind::Select(Select {
                        qualifier,
                        name,
                        backquoted,
                    }),
                );
                continue;
            }

            return (qualifier, vec![self.parse_named_selector(name, backquoted)]);
        }
    }

    fn parse_braced_selectors(&mut self) -> Vec<ImportSelector<Untyped>> {
        self.advance();
        let mut selectors = Vec::new();
        let mut names_allowed = true;
        let mut expect_selector = true;
        let mut trailing_comma = false;

        while !matches!(
            self.current().kind,
            TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Eof
        ) {
            if expect_selector && self.current().kind == TokenKind::Punctuation(Punctuation::Comma)
            {
                trailing_comma = false;
                let position = self.current_span();
                self.advance();
                if !matches!(
                    self.current().kind,
                    TokenKind::Punctuation(Punctuation::RightBrace) | TokenKind::Eof
                ) {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected an import/export selector after `,`",
                    );
                    selectors.push(self.error_selector(position));
                }
                continue;
            }

            if !expect_selector {
                if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    expect_selector = true;
                    trailing_comma =
                        self.current().kind == TokenKind::Punctuation(Punctuation::RightBrace);
                    continue;
                }
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `,` or `}` after an import/export selector",
                );
                self.recover_until(crate::RecoverySet::Statement);
                break;
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
                } else if let Some((name, backquoted)) = self.current_term_name() {
                    self.advance();
                    self.parse_named_selector(name, backquoted)
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
            expect_selector = false;
            trailing_comma = false;
        }

        if expect_selector && !trailing_comma {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected an import/export selector",
            );
            selectors.push(self.error_selector(position));
        }

        if !self.accept(TokenKind::Punctuation(Punctuation::RightBrace)) {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `}` after import/export selectors",
            );
        }
        selectors
    }

    fn parse_named_selector(
        &mut self,
        imported: dotty_core::Name,
        imported_backquoted: bool,
    ) -> ImportSelector<Untyped> {
        let renamed = if self.current_is_as() || self.current_is_arrow() {
            self.advance();
            if self.is_import_name() || self.current_is_legacy_wildcard() {
                let mark = self.mark();
                let backquoted = self.current().kind == TokenKind::BackquotedIdentifier;
                let Ok(name) = self.intern_current_term_name() else {
                    return ImportSelector {
                        imported,
                        imported_backquoted,
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
            imported_backquoted,
            renamed,
            bound: None,
        }
    }

    fn parse_wildcard_selector(&mut self) -> ImportSelector<Untyped> {
        let imported = self.names.intern("*");
        self.advance();
        ImportSelector {
            imported: dotty_core::Name::new(imported, dotty_core::Namespace::Term),
            imported_backquoted: false,
            renamed: None,
            bound: None,
        }
    }

    fn parse_given_selector(&mut self) -> ImportSelector<Untyped> {
        self.advance();
        let empty = self.names.intern("");
        let bound = if self.can_start_import_type() {
            Some(self.with_parse_kind(ParseKind::Type, |parser| parser.type_expr()))
        } else {
            None
        };
        ImportSelector {
            imported: dotty_core::Name::new(empty, dotty_core::Namespace::Term),
            imported_backquoted: false,
            renamed: None,
            bound,
        }
    }

    fn error_selector(&mut self, position: dotty_core::SourceSpan) -> ImportSelector<Untyped> {
        let empty = self.names.intern("");
        let bound = Some(self.error_expr(position));
        ImportSelector {
            imported: dotty_core::Name::new(empty, dotty_core::Namespace::Term),
            imported_backquoted: false,
            renamed: None,
            bound,
        }
    }

    fn parse_import_name(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        let Some((name, backquoted)) = self.current_term_name() else {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected an import or export qualifier",
            );
            if self.current().kind != TokenKind::Eof {
                self.advance();
            }
            return self.error_expr(position);
        };
        self.advance();
        self.alloc_from(mark, TreeKind::Ident(Ident { name, backquoted }))
    }

    fn is_import_name(&mut self) -> bool {
        self.current_term_name().is_some()
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
    use crate::statements::StatementSequenceBoundary;
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TextRange, Token, TreeKind};

    fn capture_import_tokens(source: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut offset = 0;
        while offset < source.len() {
            let bytes = source.as_bytes();
            if bytes[offset].is_ascii_whitespace() {
                offset += 1;
                continue;
            }
            if bytes[offset] == b'.' {
                tokens.push(token(
                    TokenKind::Punctuation(Punctuation::Dot),
                    offset as u32,
                    offset as u32 + 1,
                ));
                offset += 1;
                continue;
            }
            let start = offset;
            while offset < source.len()
                && !bytes[offset].is_ascii_whitespace()
                && bytes[offset] != b'.'
            {
                offset += 1;
            }
            let spelling = &source[start..offset];
            let kind = if spelling == "import" {
                TokenKind::Keyword(HardKeyword::Import)
            } else {
                TokenKind::Identifier
            };
            tokens.push(token(kind, start as u32, offset as u32));
        }
        tokens.push(token(
            TokenKind::Eof,
            source.len() as u32,
            source.len() as u32,
        ));
        tokens
    }

    fn assert_malformed_selector_list(source: &str, tokens: Vec<dotty_core::Token>) {
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, tokens, &mut names);

        parser.parse_import_clause(Location::Elsewhere);

        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

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
    fn parses_symbolic_and_ordinary_names_in_braced_import_selectors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import scala.collection.immutable.{::, List, Nil}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Punctuation(Punctuation::Dot), 12, 13),
                token(TokenKind::Identifier, 13, 23),
                token(TokenKind::Punctuation(Punctuation::Dot), 23, 24),
                token(TokenKind::Identifier, 24, 33),
                token(TokenKind::Punctuation(Punctuation::Dot), 33, 34),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 34, 35),
                token(TokenKind::Operator, 35, 37),
                token(TokenKind::Punctuation(Punctuation::Comma), 37, 38),
                token(TokenKind::Identifier, 39, 43),
                token(TokenKind::Punctuation(Punctuation::Comma), 43, 44),
                token(TokenKind::Identifier, 45, 48),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 48, 49),
                token(TokenKind::Eof, 49, 49),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        assert_eq!(import.selectors.len(), 3);
        let imported = import
            .selectors
            .iter()
            .map(|selector| selector.imported.text())
            .collect::<Vec<_>>();
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(
            imported
                .iter()
                .map(|name| names.resolve(*name))
                .collect::<Vec<_>>(),
            ["::", "List", "Nil"]
        );
    }

    #[test]
    fn preserves_backquoted_imported_selector_names() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{`+`, ::}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::BackquotedIdentifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        assert_eq!(import.selectors.len(), 2);
        assert!(import.selectors[0].imported_backquoted);
        assert!(!import.selectors[1].imported_backquoted);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn top_level_capture_checking_import_enables_the_unit_feature() {
        let source = "import language.experimental.captureChecking";
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, capture_import_tokens(source), &mut names);

        assert!(!parser.features().capture_checking);
        parser.parse_top_level_sequence(
            StatementSequenceBoundary::CompilationUnit,
            Location::Elsewhere,
        );

        assert!(parser.features().capture_checking);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parse_result_reports_capture_checking_enabled_by_global_import() {
        let source = "import language.experimental.captureChecking";
        let mut names = NameInterner::new();
        let parser = parser_for(source, capture_import_tokens(source), &mut names);

        let result = parser.source_compilation_unit();

        assert!(result.effective_features.capture_checking);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn scala_qualified_capture_checking_import_enables_the_unit_feature() {
        let source = "import scala.language.experimental.captureChecking";
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, capture_import_tokens(source), &mut names);

        parser.parse_top_level_sequence(
            StatementSequenceBoundary::CompilationUnit,
            Location::Elsewhere,
        );

        assert!(parser.features().capture_checking);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn root_qualified_capture_checking_import_enables_the_unit_feature() {
        let source = "import _root_.scala.language.experimental.captureChecking";
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, capture_import_tokens(source), &mut names);

        parser.parse_top_level_sequence(
            StatementSequenceBoundary::CompilationUnit,
            Location::Elsewhere,
        );

        assert!(parser.features().capture_checking);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn capture_checking_import_in_a_block_reports_placement_but_sets_unit_policy() {
        let source = "import language.experimental.captureChecking}";
        let mut tokens = capture_import_tokens("import language.experimental.captureChecking");
        tokens.pop();
        tokens.push(token(
            TokenKind::Punctuation(Punctuation::RightBrace),
            source.len() as u32 - 1,
            source.len() as u32,
        ));
        tokens.push(token(
            TokenKind::Eof,
            source.len() as u32,
            source.len() as u32,
        ));
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, tokens, &mut names);

        parser.parse_statement_sequence(StatementSequenceBoundary::Block(TokenKind::Punctuation(
            Punctuation::RightBrace,
        )));

        assert!(parser.features().capture_checking);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].message(),
            "this language import is only allowed at the toplevel"
        );
    }

    #[test]
    fn aliased_capture_checking_name_does_not_enable_the_feature() {
        let source = "import language.experimental.captureChecking as cc";
        let mut names = NameInterner::new();
        let mut parser = parser_for(source, capture_import_tokens(source), &mut names);

        parser.parse_top_level_sequence(
            StatementSequenceBoundary::CompilationUnit,
            Location::Elsewhere,
        );

        assert!(!parser.features().capture_checking);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn explicitly_enabled_feature_remains_enabled_without_a_language_import() {
        let mut names = NameInterner::new();
        let parser = parser_for("", vec![token(TokenKind::Eof, 0, 0)], &mut names).with_features(
            crate::ParserFeatures {
                capture_checking: true,
                ..crate::ParserFeatures::default()
            },
        );

        assert!(parser.features().capture_checking);
    }

    #[test]
    fn capture_checking_import_state_does_not_leak_to_another_parser() {
        let source = "import language.experimental.captureChecking";
        let mut names = NameInterner::new();
        let mut importing = parser_for(source, capture_import_tokens(source), &mut names);
        importing.parse_top_level_sequence(
            StatementSequenceBoundary::CompilationUnit,
            Location::Elsewhere,
        );
        assert!(importing.features().capture_checking);

        let other = parser_for("", vec![token(TokenKind::Eof, 0, 0)], &mut names);
        assert!(!other.features().capture_checking);
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
    fn diagnoses_an_empty_braced_selector_list() {
        assert_malformed_selector_list(
            "import foo.{}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
        );
    }

    #[test]
    fn diagnoses_a_leading_comma_in_braced_selectors() {
        assert_malformed_selector_list(
            "import foo.{,bar}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Punctuation(Punctuation::Comma), 12, 13),
                token(TokenKind::Identifier, 13, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
        );
    }

    #[test]
    fn diagnoses_repeated_commas_before_the_closing_brace() {
        assert_malformed_selector_list(
            "import foo.{bar,,}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Punctuation(Punctuation::Comma), 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
        );
    }

    #[test]
    fn diagnoses_a_missing_selector_between_commas_and_continues() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{bar,,baz}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Punctuation(Punctuation::Comma), 16, 17),
                token(TokenKind::Identifier, 17, 20),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an import tree");
        };
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(import.selectors.len(), 3);

        let selector_names = import
            .selectors
            .iter()
            .map(|selector| selector.imported.text())
            .collect::<Vec<_>>();
        drop(parser);
        let selector_names = selector_names
            .into_iter()
            .map(|name| names.resolve(name).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(selector_names, ["bar", "", "baz"]);
    }

    #[test]
    fn accepts_a_trailing_comma_in_braced_selectors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{bar,}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an import tree");
        };
        assert_eq!(import.selectors.len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn accepts_a_trailing_comma_in_braced_export_selectors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "export foo.{bar,}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Export), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ids = parser.parse_export_clause(Location::Elsewhere);
        let TreeKind::Export(export) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an export tree");
        };
        assert_eq!(export.selectors.len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn rejects_a_reserved_operator_as_an_imported_name_and_recovers() {
        assert_malformed_selector_list(
            "import foo.{::, =}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Punctuation(Punctuation::Comma), 14, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
        );
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
    fn accepts_a_legacy_unbraced_wildcard_import() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import p._",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Dot), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an import tree");
        };
        assert_eq!(import.selectors.len(), 1);
        let imported = import.selectors[0].imported;
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "*");
    }

    #[test]
    fn accepts_a_legacy_braced_wildcard_import() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import p.{_}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Dot), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an import tree");
        };
        assert_eq!(import.selectors.len(), 1);
        let imported = import.selectors[0].imported;
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "*");
    }

    #[test]
    fn accepts_a_legacy_unbraced_wildcard_export() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "export p._",
            vec![
                token(TokenKind::Keyword(HardKeyword::Export), 0, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Dot), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let ids = parser.parse_export_clause(Location::Elsewhere);
        let TreeKind::Export(export) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an export tree");
        };
        assert_eq!(export.selectors.len(), 1);
        let imported = export.selectors[0].imported;
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "*");
    }

    #[test]
    fn accepts_a_legacy_braced_wildcard_export() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "export p.{_}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Export), 0, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Dot), 8, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let ids = parser.parse_export_clause(Location::Elsewhere);
        let TreeKind::Export(export) = &parser.ast().get(ids[0]).kind else {
            panic!("expected an export tree");
        };
        assert_eq!(export.selectors.len(), 1);
        let imported = export.selectors[0].imported;
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
    fn parses_a_legacy_arrow_rename_in_braced_import_selectors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{bar => baz}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        let selector = import.selectors[0];
        let renamed = selector.renamed.expect("expected renamed selector");
        let TreeKind::Ident(rename) = parser.ast().get(renamed).kind else {
            panic!("expected renamed identifier");
        };

        let imported = selector.imported;
        let renamed = rename.name;
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "bar");
        assert_eq!(names.resolve(renamed.text()), "baz");
    }

    #[test]
    fn parses_a_legacy_arrow_rename_after_a_qualified_import_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.bar => baz",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Operator, 15, 17),
                token(TokenKind::Identifier, 18, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        let selector = import.selectors[0];
        let renamed = selector.renamed.expect("expected renamed selector");
        let TreeKind::Ident(rename) = parser.ast().get(renamed).kind else {
            panic!("expected renamed identifier");
        };

        let imported = selector.imported;
        let renamed = rename.name;
        assert!(matches!(
            parser.ast().get(import.expr).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(imported.text()), "bar");
        assert_eq!(names.resolve(renamed.text()), "baz");
    }

    #[test]
    fn parses_a_legacy_arrow_rename_to_wildcard_hiding() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{bar => _}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        let renamed = import.selectors[0]
            .renamed
            .expect("expected wildcard hiding identifier");
        let TreeKind::Ident(rename) = parser.ast().get(renamed).kind else {
            panic!("expected wildcard hiding identifier tree");
        };

        let renamed = rename.name;
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(renamed.text()), "_");
    }

    #[test]
    fn recovers_from_a_legacy_arrow_rename_without_a_target() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.{bar =>}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 11, 12),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };

        assert!(import.selectors[0].renamed.is_none());
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
    fn parses_a_given_selector_with_a_union_bound() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "import foo.given A | B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Import), 0, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Dot), 10, 11),
                token(TokenKind::Keyword(HardKeyword::Given), 11, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let ids = parser.parse_import_clause(Location::Elsewhere);
        let TreeKind::Import(import) = &parser.ast().get(ids[0]).kind else {
            panic!("expected import tree");
        };
        let bound = import.selectors[0]
            .bound
            .expect("expected a given selector bound");
        assert!(matches!(
            parser.ast().get(bound).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
