//! Parser support for the first, simple value-definition forms.
//!
//! Pattern definitions are added on top of this entry point in a later
//! increment.  Keeping the statement-level dispatch here means that `val`
//! and `var` do not get mistaken for unsupported top-level expressions while
//! the shared statement-sequence machinery remains grammar-agnostic.

use dotty_core::ast::{Modifier, Modifiers, TypeTree, ValDef};
use dotty_core::{HardKeyword, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, Untyped};

use crate::statements::ParsedStatement;
use crate::{Location, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_value_definition(&mut self, location: Location) -> ParsedStatement {
        let mark = self.mark();
        let is_var = self.current().kind == TokenKind::Keyword(HardKeyword::Var);
        self.advance();

        let name = match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_term_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => return self.malformed_definition(),
                }
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected an identifier after `val` or `var`",
                );
                return self.malformed_definition();
            }
        };

        let type_start = self.current().span.start();
        let has_explicit_type = is_definition_colon(self);
        let tpt = if has_explicit_type {
            self.advance();
            self.with_location(location, |parser| {
                parser.with_parse_kind(crate::ParseKind::Type, |parser| parser.simple_type())
            })
        } else {
            synthetic_type_tree(self, type_start)
        };

        let rhs = if is_bare_assignment(self) {
            self.advance();
            Some(self.with_location(location, |parser| parser.expr()))
        } else {
            if has_explicit_type && !is_definition_boundary(self.current().kind) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `=` after a value definition",
                );
            } else if !has_explicit_type {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `:` or `=` after a value definition name",
                );
            }
            None
        };

        let mut modifiers = Modifiers::default();
        if is_var {
            modifiers.modifiers.push(Modifier::Var);
        }
        let definition = self.alloc_from(
            mark,
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs,
                metadata: modifiers,
            }),
        );
        ParsedStatement::Definition(definition)
    }

    fn malformed_definition(&mut self) -> ParsedStatement {
        let position = self.current_span();
        if !is_definition_boundary(self.current().kind) {
            self.advance();
        }
        ParsedStatement::Expression(self.error_expr(position))
    }
}

fn is_definition_colon<S: dotty_core::TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    matches!(
        parser.current().kind,
        TokenKind::ColonFollow
            | TokenKind::ColonEol
            | TokenKind::ColonOp
            | TokenKind::Punctuation(dotty_core::Punctuation::Colon)
    ) && parser.current_text_is(":")
}

fn is_bare_assignment<S: dotty_core::TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    parser.current().kind == TokenKind::Operator && parser.current_text_is("=")
}

fn is_definition_boundary(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eof
            | TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Indent
            | TokenKind::Outdent
            | TokenKind::Punctuation(dotty_core::Punctuation::Semicolon)
            | TokenKind::Punctuation(dotty_core::Punctuation::RightBrace)
    )
}

fn synthetic_type_tree<S: dotty_core::TokenSource>(
    parser: &mut Parser<'_, '_, S>,
    start: u32,
) -> TreeId<Untyped> {
    let range = TextRange::new(start, start).expect("zero-width synthetic type range");
    parser.alloc(
        TreeKind::TypeTree(TypeTree),
        Some(SourceSpan::new(
            parser.source_id(),
            Span::without_point(range),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::UntypedNode;
    use dotty_core::{NameInterner, TokenKind, TreeKind};

    #[test]
    fn parses_a_val_definition_with_an_inferred_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::IntegerLiteral, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let statement = parser.parse_value_definition(Location::Elsewhere);
        let ParsedStatement::Definition(id) = statement else {
            panic!("expected a definition statement");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected ValDef");
        };
        let name_id = definition.name.as_name().text();
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::TypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(*definition.rhs.as_ref().unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(definition.metadata.modifiers.is_empty());
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "x");
    }

    #[test]
    fn parses_a_var_definition_with_the_var_modifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "var x = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Var), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::IntegerLiteral, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected ValDef");
        };
        assert_eq!(definition.metadata.modifiers, vec![Modifier::Var]);
    }

    #[test]
    fn preserves_an_explicit_type_and_allows_a_declaration_without_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x: Value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::ColonFollow, 5, 6),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected ValDef");
        };
        assert!(
            matches!(parser.ast().get(definition.tpt).kind, TreeKind::Ident(ident) if ident.name.is_type())
        );
        assert!(definition.rhs.is_none());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_missing_type_or_rhs_for_an_unannotated_declaration() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let _ = parser.parse_value_definition(Location::Elsewhere);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
    }

    #[test]
    fn definition_is_a_statement_and_the_compilation_unit_gets_unit_result() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "val x = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::IntegerLiteral, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        )
        .compilation_unit();

        let TreeKind::Block(block) = &result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(block.stats.len(), 1);
        assert!(matches!(
            result.ast.get(block.stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.expr).kind,
            TreeKind::Literal(_)
        ));
        assert!(result.diagnostics.is_empty());
    }
}
