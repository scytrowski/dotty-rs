//! Source-level `type` definitions.
//!
//! This module owns the statement boundary for simple aliases and abstract
//! type bounds. Parameterized definitions reuse `type_params.rs` and preserve
//! the source-level abstraction in `LambdaTypeTree`.

use dotty_core::ast::{LambdaTypeTree, MatchTypeTree, Modifier, TypeBoundsTree, TypeDef};
use dotty_core::{SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, TypeName, Untyped};

use crate::modifiers::DefinitionPrefix;
use crate::statements::ParsedStatement;
use crate::{Location, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_type_definition(&mut self, location: Location) -> ParsedStatement {
        let prefix = DefinitionPrefix::empty(self.mark().start());
        self.parse_type_definition_with_prefix(location, prefix)
    }

    pub(crate) fn parse_type_definition_with_prefix(
        &mut self,
        location: Location,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        let opaque = prefix.metadata.modifiers.contains(&Modifier::Opaque);
        self.advance();

        let name = self.parse_type_definition_name();
        self.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
            dotty_core::Punctuation::LeftBracket,
        ));
        let type_params_mark = if self.current().kind
            == TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket)
        {
            Some(self.mark())
        } else {
            None
        };
        let type_params = type_params_mark
            .map(|_| self.parse_type_param_clause(crate::ParamOwner::Hk))
            .unwrap_or_default();
        let empty_bounds_start = type_params
            .last()
            .and_then(|param| {
                self.ast
                    .get(*param)
                    .position
                    .map(|position| position.span().range().end())
            })
            .unwrap_or(mark.start);
        let rhs = self.parse_type_definition_rhs(location, empty_bounds_start, opaque);
        let rhs = if let Some(mark) = type_params_mark {
            let lambda_start = type_params
                .first()
                .and_then(|param| {
                    self.ast
                        .get(*param)
                        .position
                        .map(|position| position.span().range().start())
                })
                .unwrap_or(mark.start());
            let lambda = self.alloc_from(
                crate::Mark {
                    start: lambda_start,
                },
                TreeKind::LambdaTypeTree(LambdaTypeTree {
                    type_params,
                    body: rhs,
                }),
            );
            if matches!(
                self.ast.get(rhs).kind,
                TreeKind::TypeBoundsTree(TypeBoundsTree {
                    low: None,
                    high: None,
                    alias: None,
                })
            ) {
                let end = self
                    .ast
                    .get(rhs)
                    .position
                    .map(|position| position.span().range().end())
                    .unwrap_or(lambda_start);
                let range = TextRange::new(lambda_start, end).expect("lambda span is ordered");
                self.ast.get_mut(lambda).position =
                    Some(SourceSpan::new(self.source_id, Span::without_point(range)));
            } else if let Some(end) = match self.ast.get(rhs).kind {
                TreeKind::MatchTypeTree(_) => self
                    .ast
                    .get(rhs)
                    .position
                    .map(|position| position.span().range().end()),
                _ => None,
            } && let Ok(range) = TextRange::new(lambda_start, end)
            {
                self.ast.get_mut(lambda).position =
                    Some(SourceSpan::new(self.source_id, Span::without_point(range)));
            }
            lambda
        } else {
            rhs
        };
        let definition = self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs,
                metadata: prefix.metadata,
                variance: None,
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

    fn parse_type_definition_rhs(
        &mut self,
        location: Location,
        definition_start: u32,
        opaque: bool,
    ) -> TreeId<Untyped> {
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

        if self.accept_type_operator("=") {
            let owns_layout = self.current().kind == TokenKind::Indent;
            if owns_layout {
                self.advance();
            }
            if self.at_type_definition_rhs_boundary() {
                let position = self.current_span();
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type after `=`",
                );
                let error = self.error_type(position);
                if owns_layout && self.current().kind == TokenKind::Outdent {
                    self.advance();
                }
                return error;
            }
            let rhs = self.with_location(location, |parser| {
                parser.with_parse_kind(ParseKind::Type, |parser| parser.type_expr())
            });
            if owns_layout && self.current().kind == TokenKind::Outdent {
                self.advance();
            }
            if low.is_some() || high.is_some() {
                if let TreeKind::MatchTypeTree(MatchTypeTree { bound, .. }) =
                    &mut self.ast.get_mut(rhs).kind
                {
                    if low.is_none() {
                        let bound_id = high;
                        *bound = bound_id;
                        if let Some(bound_id) = bound_id {
                            self.extend_match_type_span(rhs, bound_id);
                        }
                    } else {
                        self.report(
                            ParseDiagnosticKind::UnexpectedToken,
                            "a match type alias cannot have a lower type bound",
                        );
                    }
                } else if opaque {
                    return self.alloc_opaque_type_bounds(low, high, rhs);
                } else {
                    self.report(
                        ParseDiagnosticKind::UnexpectedToken,
                        "only a match type alias can combine a type bound with `=`",
                    );
                }
            }
            return rhs;
        }

        if low.is_none() && high.is_none() {
            if opaque {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `=` after an opaque type definition",
                );
            }
            // Dotty represents an abstract declaration's empty bounds at the
            // start of the type definition, rather than after its name.
            return self.synthetic_type_bounds(definition_start);
        }

        let bounds = self.alloc_from(
            crate::Mark {
                start: bounds_start,
            },
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low,
                high,
                alias: None,
            }),
        );
        if opaque {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=` after an opaque type bound",
            );
        }
        bounds
    }

    fn alloc_opaque_type_bounds(
        &mut self,
        low: Option<TreeId<Untyped>>,
        high: Option<TreeId<Untyped>>,
        alias: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        let start = low
            .or(high)
            .and_then(|bound| self.ast.get(bound).position)
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| {
                self.ast
                    .get(alias)
                    .position
                    .map_or(self.mark().start(), |position| {
                        position.span().range().start()
                    })
            });
        let end = self
            .ast
            .get(alias)
            .position
            .map(|position| position.span().range().end())
            .unwrap_or(start);
        let bounds = self.alloc_from(
            crate::Mark { start },
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low,
                high,
                alias: Some(alias),
            }),
        );
        if let Ok(range) = TextRange::new(start, end) {
            self.ast.get_mut(bounds).position =
                Some(SourceSpan::new(self.source_id, Span::without_point(range)));
        }
        bounds
    }

    fn extend_match_type_span(&mut self, match_type: TreeId<Untyped>, bound: TreeId<Untyped>) {
        let Some(bound_position) = self.ast.get(bound).position else {
            return;
        };
        let Some(match_type_position) = self.ast.get(match_type).position else {
            return;
        };
        let match_end = match &self.ast.get(match_type).kind {
            TreeKind::MatchTypeTree(match_type_tree) => match_type_tree
                .cases
                .last()
                .and_then(|case| self.ast.get(*case).position)
                .map(|position| position.span().range().end())
                .unwrap_or(match_type_position.span().range().end()),
            _ => match_type_position.span().range().end(),
        };
        let range = TextRange::new(bound_position.span().range().start(), match_end);
        let Ok(range) = range else {
            return;
        };
        self.ast.get_mut(match_type).position =
            Some(SourceSpan::new(self.source_id, Span::without_point(range)));
    }

    fn parse_type_definition_bound_type(&mut self, location: Location) -> TreeId<Untyped> {
        self.with_location(location, |parser| {
            parser.with_parse_kind(ParseKind::Type, |parser| parser.type_expr())
        })
    }

    fn at_type_definition_rhs_boundary(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Eof
                | TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Outdent
                | TokenKind::Punctuation(
                    dotty_core::Punctuation::RightBrace | dotty_core::Punctuation::Semicolon
                )
        )
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
    use dotty_core::ast::{LambdaTypeTree, Modifier, TypeBoundsTree, TypeDef};
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
    fn parses_an_opaque_type_alias_with_opaque_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "opaque type UserId = Long",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Keyword(HardKeyword::Type), 7, 11),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::Identifier, 21, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition statement");
        };
        let TreeKind::TypeDef(TypeDef {
            ref metadata, rhs, ..
        }) = parser.ast().get(id).kind
        else {
            panic!("expected TypeDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Opaque]);
        assert!(matches!(parser.ast().get(rhs).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_parameterized_opaque_type_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "opaque type Id[A] = A",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Keyword(HardKeyword::Type), 7, 11),
                token(TokenKind::Identifier, 12, 14),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    14,
                    15,
                ),
                token(TokenKind::Identifier, 15, 16),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    16,
                    17,
                ),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition statement");
        };
        let TreeKind::TypeDef(TypeDef {
            ref metadata, rhs, ..
        }) = parser.ast().get(id).kind
        else {
            panic!("expected TypeDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Opaque]);
        let TreeKind::LambdaTypeTree(LambdaTypeTree {
            ref type_params,
            body,
        }) = parser.ast().get(rhs).kind
        else {
            panic!("expected LambdaTypeTree");
        };
        assert_eq!(type_params.len(), 1);
        assert!(matches!(parser.ast().get(body).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_bounds_and_alias_for_an_opaque_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "opaque type Both >: Lower <: Upper = Impl",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Keyword(HardKeyword::Type), 7, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 25),
                token(TokenKind::Operator, 26, 28),
                token(TokenKind::Identifier, 29, 34),
                token(TokenKind::Operator, 35, 36),
                token(TokenKind::Identifier, 37, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition statement");
        };
        let TreeKind::TypeDef(TypeDef {
            ref metadata, rhs, ..
        }) = parser.ast().get(id).kind
        else {
            panic!("expected TypeDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Opaque]);
        let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, alias }) =
            parser.ast().get(rhs).kind
        else {
            panic!("expected TypeBoundsTree");
        };
        assert!(low.is_some() && high.is_some() && alias.is_some());
        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(20, 41).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_an_opaque_bound_without_swallowing_the_next_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "opaque type Broken <: Upper\ntype Next = Impl",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Keyword(HardKeyword::Type), 7, 11),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 27),
                token(TokenKind::Newline, 27, 28),
                token(TokenKind::Keyword(HardKeyword::Type), 28, 32),
                token(TokenKind::Identifier, 33, 37),
                token(TokenKind::Operator, 38, 39),
                token(TokenKind::Identifier, 40, 44),
                token(TokenKind::Eof, 44, 44),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(_) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected the malformed opaque definition");
        };
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Newline);

        parser.advance();
        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected the following type definition");
        };
        assert!(matches!(parser.ast().get(id).kind, TreeKind::TypeDef(_)));
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
    fn parses_a_parameterized_type_alias_as_a_lambda_type_tree() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type F[A] = A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    6,
                    7,
                ),
                token(TokenKind::Identifier, 7, 8),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    8,
                    9,
                ),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
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
        let TreeKind::LambdaTypeTree(LambdaTypeTree {
            ref type_params,
            body,
        }) = parser.ast().get(rhs).kind
        else {
            panic!("expected LambdaTypeTree");
        };
        assert_eq!(type_params.len(), 1);
        assert!(
            matches!(parser.ast().get(body).kind, TreeKind::Ident(ident) if ident.name.is_type())
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_multiple_type_parameters_in_a_type_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type EitherLike[A, B] = A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 15),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    15,
                    16,
                ),
                token(TokenKind::Identifier, 16, 17),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::Comma),
                    17,
                    18,
                ),
                token(TokenKind::Identifier, 19, 20),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    20,
                    21,
                ),
                token(TokenKind::Operator, 22, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Eof, 25, 25),
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
        let TreeKind::LambdaTypeTree(LambdaTypeTree {
            ref type_params, ..
        }) = parser.ast().get(rhs).kind
        else {
            panic!("expected LambdaTypeTree");
        };
        assert_eq!(type_params.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn wraps_parameterized_abstract_bounds_in_a_lambda_type_tree() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type F[A] <: Upper",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    6,
                    7,
                ),
                token(TokenKind::Identifier, 7, 8),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    8,
                    9,
                ),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 18),
                token(TokenKind::Eof, 18, 18),
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
        let TreeKind::LambdaTypeTree(LambdaTypeTree { body, .. }) = parser.ast().get(rhs).kind
        else {
            panic!("expected LambdaTypeTree");
        };
        assert!(matches!(
            parser.ast().get(body).kind,
            TreeKind::TypeBoundsTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn attaches_an_upper_bound_to_a_parameterized_match_type_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type Elem[X] <: Any = X match { case _ => X }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    9,
                    10,
                ),
                token(TokenKind::Identifier, 10, 11),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    11,
                    12,
                ),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::Operator, 20, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Keyword(HardKeyword::Match), 24, 29),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBrace),
                    30,
                    31,
                ),
                token(TokenKind::Keyword(HardKeyword::Case), 32, 36),
                token(TokenKind::Identifier, 37, 38),
                token(TokenKind::Operator, 39, 41),
                token(TokenKind::Identifier, 42, 43),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBrace),
                    44,
                    45,
                ),
                token(TokenKind::Eof, 45, 45),
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
        let TreeKind::LambdaTypeTree(LambdaTypeTree { body, .. }) = parser.ast().get(rhs).kind
        else {
            panic!("expected LambdaTypeTree");
        };
        let TreeKind::MatchTypeTree(MatchTypeTree { bound, .. }) = parser.ast().get(body).kind
        else {
            panic!("expected MatchTypeTree");
        };
        assert!(bound.is_some());
        assert!(matches!(
            parser.ast().get(bound.unwrap()).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser.ast().get(body).position.unwrap().span().range(),
            TextRange::new(16, 43).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_lower_bound_on_a_match_type_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type Elem >: Nothing = X match { case _ => X }",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 20),
                token(TokenKind::Operator, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Keyword(HardKeyword::Match), 25, 30),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBrace),
                    31,
                    32,
                ),
                token(TokenKind::Keyword(HardKeyword::Case), 33, 37),
                token(TokenKind::Identifier, 38, 39),
                token(TokenKind::Operator, 40, 42),
                token(TokenKind::Identifier, 43, 44),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBrace),
                    45,
                    46,
                ),
                token(TokenKind::Eof, 46, 46),
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
        let TreeKind::MatchTypeTree(MatchTypeTree { bound, .. }) = parser.ast().get(rhs).kind
        else {
            panic!("expected MatchTypeTree");
        };
        assert!(bound.is_none());
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken })
        );
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
            TextRange::new(0, 0).unwrap()
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

    #[test]
    fn recovers_from_a_type_definition_without_a_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(_) = parser.parse_type_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
    }

    #[test]
    fn recovers_from_a_type_alias_without_a_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type A =",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(_) = parser.parse_type_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
    }

    #[test]
    fn a_missing_type_alias_rhs_preserves_a_following_statement() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "type A =\nvalue",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Newline, 8, 9),
                token(TokenKind::Identifier, 9, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        )
        .compilation_unit();

        let TreeKind::Block(ref block) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(block.stats.len(), 1);
        assert!(matches!(
            result.ast.get(block.stats[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
    }

    #[test]
    fn malformed_unnamed_erased_function_type_preserves_a_following_definition() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "type A = (erased A => B\ntype B = C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    9,
                    10,
                ),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Newline, 23, 24),
                token(TokenKind::Keyword(HardKeyword::Type), 24, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Operator, 31, 32),
                token(TokenKind::Identifier, 33, 34),
                token(TokenKind::Eof, 34, 34),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        })
        .compilation_unit();

        let TreeKind::Block(ref block) = result.ast.get(result.root).kind else {
            panic!("expected a compilation-unit block");
        };
        assert_eq!(block.stats.len(), 2);
        assert!(matches!(
            result.ast.get(block.stats[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.stats[1]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
    }

    #[test]
    fn malformed_unnamed_erased_context_function_type_preserves_a_following_definition() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "type A = (erased A ?=> B\ntype B = C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    9,
                    10,
                ),
                token(TokenKind::Identifier, 10, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Newline, 24, 25),
                token(TokenKind::Keyword(HardKeyword::Type), 25, 29),
                token(TokenKind::Identifier, 30, 31),
                token(TokenKind::Operator, 32, 33),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::Eof, 35, 35),
            ],
            &mut names,
        )
        .with_features(crate::ParserFeatures {
            erased_definitions: true,
            ..crate::ParserFeatures::default()
        })
        .compilation_unit();

        let TreeKind::Block(ref block) = result.ast.get(result.root).kind else {
            panic!("expected a compilation-unit block");
        };
        assert_eq!(block.stats.len(), 2);
        assert!(matches!(
            result.ast.get(block.stats[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.stats[1]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
    }

    #[test]
    fn parses_a_type_alias_inside_a_layout_region() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "type A =\n  B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Indent, 8, 8),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Outdent, 9, 9),
                token(TokenKind::Eof, 9, 9),
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
        assert!(matches!(
            parser.ast().get(rhs).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }
}
