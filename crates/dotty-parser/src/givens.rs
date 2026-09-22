//! Source-level Scala 3.9 `given` definitions.
//!
//! This module starts with the alias form. Structural instances and
//! conditional signatures are layered on the same dispatch in later
//! increments.

use dotty_core::ast::{DefDef, Modifier, ValDef};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::modifiers::DefinitionPrefix;
use crate::names::anonymous_term_name;
use crate::statements::ParsedStatement;
use crate::{Location, ParamOwner, ParseDiagnosticKind, ParseKind, Parser};

struct GivenSignature {
    name: dotty_core::TermName,
    type_params: Vec<TreeId<Untyped>>,
    value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
    tpt: TreeId<Untyped>,
    rhs: TreeId<Untyped>,
    metadata: dotty_core::ast::Modifiers,
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_given_definition(&mut self, location: Location) -> ParsedStatement {
        let prefix = DefinitionPrefix::empty(self.mark().start());
        self.parse_given_definition_with_prefix(location, prefix)
    }

    pub(crate) fn parse_given_definition_with_prefix(
        &mut self,
        location: Location,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        self.expect(TokenKind::Keyword(HardKeyword::Given));

        let name = if self.starts_named_given() {
            let name = self
                .intern_current_term_name()
                .unwrap_or_else(|_| anonymous_term_name(self.names));
            self.advance();
            self.advance(); // `:`
            name
        } else {
            anonymous_term_name(self.names)
        };

        let type_params = if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket)
        {
            let params = self.parse_type_param_clause(ParamOwner::Given);
            self.expect_arrow();
            params
        } else {
            Vec::new()
        };
        let value_param_clauses =
            if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
                let clauses = self.parse_term_param_clauses(ParamOwner::Given);
                self.expect_arrow();
                clauses
            } else {
                Vec::new()
            };

        let tpt = self.with_location(location, |parser| {
            parser.with_parse_kind(ParseKind::Type, |parser| parser.simple_type())
        });
        let mut metadata = prefix.metadata;
        if !metadata.modifiers.contains(&Modifier::Given) {
            metadata.modifiers.push(Modifier::Given);
        }
        if !metadata.modifiers.contains(&Modifier::Final) {
            metadata.modifiers.push(Modifier::Final);
        }

        if !self.current_is_bare_assignment() {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=` after a given type",
            );
            let rhs = self.error_expr(self.current_span());
            return self.alloc_given_definition(
                mark,
                GivenSignature {
                    name,
                    type_params,
                    value_param_clauses,
                    tpt,
                    rhs,
                    metadata,
                },
            );
        }

        self.advance();
        if type_params.is_empty()
            && value_param_clauses.is_empty()
            && !metadata.modifiers.contains(&Modifier::Inline)
            && !metadata.modifiers.contains(&Modifier::Erased)
        {
            metadata.modifiers.push(Modifier::Lazy);
        }
        let rhs = self.with_location(location, |parser| parser.expr());
        self.alloc_given_definition(
            mark,
            GivenSignature {
                name,
                type_params,
                value_param_clauses,
                tpt,
                rhs,
                metadata,
            },
        )
    }

    fn alloc_given_definition(
        &mut self,
        mark: crate::Mark,
        signature: GivenSignature,
    ) -> ParsedStatement {
        if signature.type_params.is_empty() && signature.value_param_clauses.is_empty() {
            ParsedStatement::Definition(self.alloc_from(
                mark,
                TreeKind::ValDef(ValDef {
                    name: signature.name,
                    tpt: signature.tpt,
                    rhs: Some(signature.rhs),
                    metadata: signature.metadata,
                }),
            ))
        } else {
            ParsedStatement::Definition(self.alloc_from(
                mark,
                TreeKind::DefDef(DefDef {
                    name: signature.name,
                    type_params: signature.type_params,
                    value_param_clauses: signature.value_param_clauses,
                    tpt: signature.tpt,
                    rhs: Some(signature.rhs),
                    metadata: signature.metadata,
                }),
            ))
        }
    }

    fn starts_named_given(&mut self) -> bool {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return false;
        }
        let next = self.cursor.lookahead(1).clone();
        matches!(
            next.kind,
            TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::ColonOp
                | TokenKind::Punctuation(Punctuation::Colon)
        ) && self.token_text(&next).ok() == Some(":")
    }

    fn expect_arrow(&mut self) {
        if self.current_is_arrow() {
            self.advance();
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=>` in a given signature",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, TextRange};

    #[test]
    fn parses_anonymous_given_alias_with_the_empty_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given Config = makeConfig",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Operator, 13, 14),
                token(TokenKind::Identifier, 15, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected given definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a value definition");
        };

        let name_id = definition.name.as_name().text();
        assert!(definition.metadata.modifiers.contains(&Modifier::Given));
        assert!(definition.metadata.modifiers.contains(&Modifier::Final));
        assert!(definition.metadata.modifiers.contains(&Modifier::Lazy));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 25).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "");
    }
}
