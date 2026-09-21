//! Source-level `type` definitions.
//!
//! This module owns the statement boundary for simple aliases and abstract
//! type bounds. Parameterized definitions are layered on top of the same
//! entry point and reuse `type_params.rs` in a later increment.

use dotty_core::ast::{Modifiers, TypeBoundsTree, TypeDef};
use dotty_core::{TokenKind, TreeId, TreeKind, TypeName, Untyped};

use crate::statements::ParsedStatement;
use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_type_definition(&mut self, location: Location) -> ParsedStatement {
        let mark = self.mark();
        self.advance();

        let name = self.parse_type_definition_name();
        let rhs = self.parse_type_definition_rhs(location);
        let definition = self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs,
                metadata: Modifiers::default(),
            }),
        );
        ParsedStatement::Definition(definition)
    }

    fn parse_type_definition_name(&mut self) -> TypeName {
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_type_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => self.missing_type_definition_name(),
                }
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type name after `type`",
                );
                self.missing_type_definition_name()
            }
        }
    }

    fn missing_type_definition_name(&mut self) -> TypeName {
        TypeName::new(self.names.intern("$missing_type"))
    }

    fn parse_type_definition_rhs(&mut self, location: Location) -> TreeId<Untyped> {
        if self.accept_type_operator("=") {
            return self.with_location(location, |parser| {
                parser.with_parse_kind(ParseKind::Type, |parser| parser.simple_type())
            });
        }

        let bounds_start = self.current().span.start();
        let low = if self.accept_type_operator(">:") {
            Some(self.parse_type_definition_bound_type(location))
        } else {
            None
        };
        let high = if self.accept_type_operator("<:") {
            Some(self.parse_type_definition_bound_type(location))
        } else {
            None
        };

        if low.is_none() && high.is_none() {
            return self.synthetic_type_bounds(self.last_real_token_end);
        }

        self.alloc_from(
            crate::Mark {
                start: bounds_start,
            },
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low,
                high,
                alias: None,
            }),
        )
    }

    fn parse_type_definition_bound_type(&mut self, location: Location) -> TreeId<Untyped> {
        self.with_location(location, |parser| {
            parser.with_parse_kind(ParseKind::Type, |parser| parser.simple_type())
        })
    }

    fn accept_type_operator(&mut self, expected: &str) -> bool {
        if matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::ColonOp
        ) && self.current_text_is(expected)
        {
            self.advance();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{TypeBoundsTree, TypeDef};
    use dotty_core::{HardKeyword, NameInterner, TextRange};

    #[test]
    fn parses_a_simple_type_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type A = B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_type_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::TypeDef(TypeDef { name, rhs, .. }) = parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert!(name.as_name().is_type());
        assert!(
            matches!(parser.ast().get(rhs).kind, TreeKind::Ident(ident) if ident.name.is_type())
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 10).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn statement_dispatch_classifies_type_as_a_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type A = B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected type definition statement");
        };
        assert!(matches!(parser.ast().get(id).kind, TreeKind::TypeDef(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_abstract_type_declaration_with_empty_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_type_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, alias }) =
            parser.ast().get(rhs).kind
        else {
            panic!("expected TypeBoundsTree");
        };
        assert!(low.is_none() && high.is_none() && alias.is_none());
        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(6, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_lower_upper_and_two_sided_type_bounds() {
        for (source, tokens, expect_low, expect_high) in [
            (
                "type A <: Upper",
                vec![
                    token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                    token(TokenKind::Identifier, 5, 6),
                    token(TokenKind::Operator, 7, 9),
                    token(TokenKind::Identifier, 10, 15),
                    token(TokenKind::Eof, 15, 15),
                ],
                false,
                true,
            ),
            (
                "type A >: Lower",
                vec![
                    token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                    token(TokenKind::Identifier, 5, 6),
                    token(TokenKind::Operator, 7, 9),
                    token(TokenKind::Identifier, 10, 15),
                    token(TokenKind::Eof, 15, 15),
                ],
                true,
                false,
            ),
            (
                "type A >: Lower <: Upper",
                vec![
                    token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                    token(TokenKind::Identifier, 5, 6),
                    token(TokenKind::Operator, 7, 9),
                    token(TokenKind::Identifier, 10, 15),
                    token(TokenKind::Operator, 16, 18),
                    token(TokenKind::Identifier, 19, 24),
                    token(TokenKind::Eof, 24, 24),
                ],
                true,
                true,
            ),
        ] {
            let mut names = NameInterner::new();
            let mut parser = parser_for(source, tokens, &mut names);
            let ParsedStatement::Definition(id) = parser.parse_type_definition(Location::Elsewhere)
            else {
                panic!("expected a definition statement");
            };
            let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(id).kind else {
                panic!("expected TypeDef");
            };
            let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, alias }) =
                parser.ast().get(rhs).kind
            else {
                panic!("expected TypeBoundsTree");
            };
            assert_eq!(low.is_some(), expect_low);
            assert_eq!(high.is_some(), expect_high);
            assert!(alias.is_none());
            assert!(parser.diagnostics().is_empty(), "diagnostics for {source}");
        }
    }

    #[test]
    fn parses_a_backquoted_type_definition_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type `match` = A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::BackquotedIdentifier, 5, 12),
                token(TokenKind::Operator, 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_type_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::TypeDef(TypeDef { name, .. }) = parser.ast().get(id).kind else {
            panic!("expected TypeDef");
        };
        assert!(name.as_name().is_type());
        assert_eq!(parser.names.resolve(name.as_name().text()), "match");
        assert!(parser.diagnostics().is_empty());
    }
}
