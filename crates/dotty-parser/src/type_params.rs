use dotty_core::ast::{Modifiers, TypeBoundsTree, TypeDef};
use dotty_core::{
    Punctuation, SourceSpan, Span, TextRange, TokenKind, TreeId, TreeKind, TypeName, Untyped,
};

use crate::{ParamOwner, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the initial reusable subset of a Scala type-parameter clause.
    ///
    /// The owner is kept explicit even though the first consumer is a
    /// polymorphic function literal. Future definition parsers can reuse this
    /// entry point without introducing a second square-bracket grammar.
    pub(crate) fn parse_type_param_clause(&mut self, owner: ParamOwner) -> Vec<TreeId<Untyped>> {
        self.with_param_owner(Some(owner), |parser| parser.type_param_clause())
    }

    fn type_param_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftBracket));
        let mut params = Vec::new();

        if self.current().kind == TokenKind::Punctuation(Punctuation::RightBracket) {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected a type parameter between `[` and `]`",
            );
        }

        while self.current().kind != TokenKind::Eof
            && self.current().kind != TokenKind::Punctuation(Punctuation::RightBracket)
        {
            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type parameter after `,`",
                );
                continue;
            }

            let checkpoint = self.cursor.checkpoint();
            params.push(self.type_param());
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a type parameter",
                );
                break;
            }

            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                continue;
            }
            if self.current().kind == TokenKind::Punctuation(Punctuation::RightBracket)
                || self.current().kind == TokenKind::Eof
                || self.current_is_arrow()
            {
                break;
            }

            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `,` or `]` after a type parameter",
            );
            self.recover_type_param_clause();
            if self.current_is_arrow() {
                break;
            }
        }

        self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
        params
    }

    fn type_param(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let variance = if self.current().kind == TokenKind::Operator
            && matches!(self.current_text().ok(), Some("+" | "-"))
        {
            let variance = self.current_text().ok().unwrap_or_default().to_owned();
            self.advance();
            Some(variance)
        } else {
            None
        };

        let name = self.parse_type_param_name();
        if variance.is_some() && self.context.param_owner == Some(ParamOwner::Type) {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "variance is not allowed for a polymorphic function type parameter",
            );
        }

        let explicit_bounds_start = self.current().span.start();
        let low = if self.accept_operator(">:") {
            Some(self.parse_bound_type())
        } else {
            None
        };
        let high = if self.accept_operator("<:") {
            Some(self.parse_bound_type())
        } else {
            None
        };
        let bounds = if low.is_some() || high.is_some() {
            self.alloc_from(
                crate::Mark {
                    start: explicit_bounds_start,
                },
                TreeKind::TypeBoundsTree(TypeBoundsTree {
                    low,
                    high,
                    alias: None,
                }),
            )
        } else {
            self.synthetic_type_bounds(mark.start())
        };

        self.alloc_from(
            mark,
            TreeKind::TypeDef(TypeDef {
                name,
                rhs: bounds,
                metadata: Modifiers::default(),
            }),
        )
    }

    fn parse_type_param_name(&mut self) -> TypeName {
        if self.current().kind == TokenKind::Identifier && self.current_text_is("_") {
            self.advance();
            return self.fresh_wildcard_type_name();
        }

        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_type_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => self.missing_type_param_name(),
                }
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type parameter name",
                );
                self.missing_type_param_name()
            }
        }
    }

    fn missing_type_param_name(&mut self) -> TypeName {
        if !matches!(
            self.current().kind,
            TokenKind::Eof | TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightBracket)
        ) && !self.current_is_arrow()
        {
            self.advance();
        }
        self.fresh_wildcard_type_name()
    }

    fn parse_bound_type(&mut self) -> TreeId<Untyped> {
        self.with_parse_kind(ParseKind::Type, |parser| parser.simple_type())
    }

    fn accept_operator(&mut self, expected: &str) -> bool {
        if self.current().kind == TokenKind::Operator && self.current_text_is(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn recover_type_param_clause(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Eof | TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightBracket)
        ) && !self.current_is_arrow()
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn fresh_wildcard_type_name(&mut self) -> TypeName {
        let index = self.next_wildcard_type_param;
        self.next_wildcard_type_param = self.next_wildcard_type_param.saturating_add(1);
        let name = format!("$type_wildcard_{index}");
        TypeName::new(self.names.intern(&name))
    }

    fn synthetic_type_bounds(&mut self, start: u32) -> TreeId<Untyped> {
        let range = TextRange::new(start, start).expect("zero-width type bounds range");
        self.alloc(
            TreeKind::TypeBoundsTree(TypeBoundsTree {
                low: None,
                high: None,
                alias: None,
            }),
            Some(SourceSpan::new(self.source_id, Span::without_point(range))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::ast::{TypeBoundsTree, TypeDef};
    use dotty_core::{NameInterner, TextRange, Token, TokenValue};

    #[test]
    fn parses_an_unbounded_type_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 1);
        let TreeKind::TypeDef(TypeDef { name, rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type definition");
        };
        assert!(name.as_name().is_type());
        let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, alias }) =
            parser.ast().get(rhs).kind
        else {
            panic!("expected type bounds");
        };
        assert!(low.is_none() && high.is_none() && alias.is_none());
        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(1, 1).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_multiple_type_parameters_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A, B]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 2);
        let parameter_names = params
            .iter()
            .map(|param| match parser.ast().get(*param).kind {
                TreeKind::TypeDef(TypeDef { name, .. }) => name,
                _ => panic!("expected a type definition"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            parser.names.resolve(parameter_names[0].as_name().text()),
            "A"
        );
        assert_eq!(
            parser.names.resolve(parameter_names[1].as_name().text()),
            "B"
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_simple_lower_and_upper_type_bounds() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A >: Lower <: Upper]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 11),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(12, 14).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        let TreeKind::TypeDef(TypeDef { rhs, .. }) = parser.ast().get(params[0]).kind else {
            panic!("expected a type definition");
        };
        let TreeKind::TypeBoundsTree(TypeBoundsTree { low, high, .. }) = parser.ast().get(rhs).kind
        else {
            panic!("expected type bounds");
        };
        assert!(low.is_some() && high.is_some());
        assert_eq!(
            parser.ast().get(rhs).position.unwrap().span().range(),
            TextRange::new(3, 20).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_unsupported_applied_type_in_a_bound_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[A <: Foo & Bar]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                Token {
                    kind: TokenKind::Operator,
                    span: TextRange::new(3, 5).unwrap(),
                    value: TokenValue::None,
                },
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);

        assert_eq!(params.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message()
                .contains("expected `,` or `]` after a type parameter")
        }));
    }

    #[test]
    fn malformed_type_parameter_clause_recovers_at_the_closing_bracket() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[,A]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::Comma), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert_eq!(params.len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn empty_type_parameter_clause_reports_a_missing_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "[]",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let params = parser.parse_type_param_clause(ParamOwner::Type);
        assert!(params.is_empty());
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.message().contains("expected a type parameter") })
        );
    }
}
