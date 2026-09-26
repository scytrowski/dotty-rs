use dotty_core::ast::{Modifier, PackageDef};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::modifiers::DefinitionPrefix;
use crate::references::{QualifiedReferenceError, ReferenceNamespace};
use crate::statements::{ParsedStatement, StatementSequenceBoundary};
use crate::{Location, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_package_definition(&mut self, location: Location) -> ParsedStatement {
        let mark = self.mark();
        let package_span = self.current_span();
        self.advance();

        if self.current().kind == TokenKind::Keyword(HardKeyword::Object) {
            if !matches!(location, Location::Elsewhere | Location::InPackageBody) {
                self.report_at(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    package_span,
                    "package object definitions are only allowed at top level or in a package body",
                );
                let _ = self.parse_object_definition(Location::InBlock);
                return ParsedStatement::Expression(self.error_expr(package_span));
            }

            let mut prefix = DefinitionPrefix::empty(mark.start());
            prefix.metadata.modifiers.push(Modifier::PackageObject);
            return self.parse_object_definition_with_prefix(prefix);
        }

        let name = self.parse_package_name();
        let stats = self.parse_package_body();
        ParsedStatement::Definition(
            self.alloc_from(mark, TreeKind::PackageDef(PackageDef { name, stats })),
        )
    }

    fn parse_package_name(&mut self) -> TreeId<Untyped> {
        match self.parse_qualified_reference(ReferenceNamespace::Term) {
            Ok(tree) => tree,
            Err(QualifiedReferenceError::MissingInitial) => {
                let position = self.current_span();
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a package name",
                );
                if self.current().kind != TokenKind::Eof {
                    self.advance();
                }
                self.error_expr(position)
            }
            Err(QualifiedReferenceError::MissingSegment) => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a package name after `.`",
                );
                self.error_expr(self.current_span())
            }
        }
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

        let stats = self.with_location(Location::InPackageBody, |parser| {
            parser.with_block_end(Some(end), |parser| {
                parser.parse_top_level_sequence(
                    StatementSequenceBoundary::Block(end),
                    Location::InPackageBody,
                )
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
        self.parse_top_level_sequence(
            StatementSequenceBoundary::CompilationUnit,
            Location::InPackageBody,
        )
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

    #[test]
    fn allows_package_objects_in_braced_package_bodies() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "package demo { package object foo {} }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 13, 14),
                token(TokenKind::Keyword(HardKeyword::Package), 15, 22),
                token(TokenKind::Keyword(HardKeyword::Object), 23, 29),
                token(TokenKind::Identifier, 30, 33),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 34, 35),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 35, 36),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 37, 38),
                token(TokenKind::Eof, 38, 38),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_package_definition(Location::Elsewhere)
        else {
            panic!("expected outer package definition");
        };
        let TreeKind::PackageDef(package) = &parser.ast().get(id).kind else {
            panic!("expected package tree");
        };
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) =
            &parser.ast().get(package.stats[0]).kind
        else {
            panic!("expected package object module definition");
        };
        assert!(module.metadata.modifiers.contains(&Modifier::PackageObject));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_empty_package_object_as_a_package_marked_module() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "package object foo {}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Keyword(HardKeyword::Object), 8, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 19, 20),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_package_definition(Location::Elsewhere)
        else {
            panic!("expected package object definition");
        };
        let (package_name, template_id, has_package_object_marker) =
            match &parser.ast().get(id).kind {
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) => (
                    module.name,
                    module.template,
                    module.metadata.modifiers.contains(&Modifier::PackageObject),
                ),
                _ => panic!("expected package object to remain a ModuleDef"),
            };
        assert!(has_package_object_marker);
        let TreeKind::Template(template) = &parser.ast().get(template_id).kind else {
            panic!("expected module template");
        };
        assert!(template.body.is_empty());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 21).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(package_name.as_name().text()), "foo");
    }

    #[test]
    fn preserves_package_object_members_and_full_source_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "package object foo { def answer = 42 }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Keyword(HardKeyword::Object), 8, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Def), 21, 24),
                token(TokenKind::Identifier, 25, 31),
                token(TokenKind::Operator, 32, 33),
                token(TokenKind::IntegerLiteral, 34, 36),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 37, 38),
                token(TokenKind::Eof, 38, 38),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_package_definition(Location::Elsewhere)
        else {
            panic!("expected package object definition");
        };
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(module)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected package object ModuleDef");
        };
        let TreeKind::Template(template) = &parser.ast().get(module.template).kind else {
            panic!("expected package object template");
        };
        assert_eq!(template.body.len(), 1);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 38).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn missing_package_object_name_does_not_swallow_the_next_top_level_object() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "package object {}\nobject Next {}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Keyword(HardKeyword::Object), 8, 14),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 16, 17),
                token(TokenKind::Newline, 17, 18),
                token(TokenKind::Keyword(HardKeyword::Object), 18, 24),
                token(TokenKind::Identifier, 25, 29),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 30, 31),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let result = parser.source_compilation_unit();

        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected the source package root");
        };
        assert_eq!(package.stats.len(), 2);
        assert!(matches!(
            result.ast.get(package.stats[0]).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(_))
        ));
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(next)) =
            &result.ast.get(package.stats[1]).kind
        else {
            panic!("expected following object definition to survive recovery");
        };
        assert_eq!(names.resolve(next.name.as_name().text()), "Next");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind() == crate::ParseDiagnosticKind::ExpectedToken)
        );
    }

    #[test]
    fn package_object_inside_a_template_is_rejected_and_recovered() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "object Outer { package object Inner {} }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Object), 0, 6),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 13, 14),
                token(TokenKind::Keyword(HardKeyword::Package), 15, 22),
                token(TokenKind::Keyword(HardKeyword::Object), 23, 29),
                token(TokenKind::Identifier, 30, 35),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 36, 37),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 37, 38),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 39, 40),
                token(TokenKind::Eof, 40, 40),
            ],
            &mut names,
        );

        let result = parser.source_compilation_unit();
        let TreeKind::PackageDef(root) = &result.ast.get(result.root).kind else {
            panic!("expected source package root");
        };
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(outer)) =
            &result.ast.get(root.stats[0]).kind
        else {
            panic!("expected outer object definition");
        };
        let TreeKind::Template(template) = &result.ast.get(outer.template).kind else {
            panic!("expected outer template");
        };
        assert!(matches!(
            result.ast.get(template.body[0]).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Error(_))
        ));
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            crate::ParseDiagnosticKind::UnsupportedSyntax
        );
    }

    #[test]
    fn package_object_in_a_block_is_rejected_without_leaving_its_body_unparsed() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "package object Inner {}",
            vec![
                token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                token(TokenKind::Keyword(HardKeyword::Object), 8, 14),
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 21, 22),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 22, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ParsedStatement::Expression(error) = parser.parse_statement(Location::InBlock) else {
            panic!("expected an error expression in a regular block");
        };
        assert!(matches!(
            parser.ast().get(error).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Error(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            crate::ParseDiagnosticKind::UnsupportedSyntax
        );
    }
}
