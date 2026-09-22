//! Source-level Scala 3.9 `given` definitions.
//!
//! This module starts with the alias form. Structural instances and
//! conditional signatures are layered on the same dispatch in later
//! increments.

use dotty_core::ast::{DefDef, Modifier, ModuleDef, TypeDef, ValDef};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::modifiers::DefinitionPrefix;
use crate::names::{anonymous_term_name, anonymous_type_name};
use crate::statements::ParsedStatement;
use crate::{Location, ParamOwner, ParseDiagnosticKind, ParseKind, Parser};

struct GivenSignature {
    name: dotty_core::TermName,
    type_params: Vec<TreeId<Untyped>>,
    value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
    method_like: bool,
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
        let mut method_like = !type_params.is_empty()
            || prefix
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Inline)
            || prefix
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Erased);
        let value_param_clauses =
            if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
                method_like = true;
                let clauses = self
                    .parse_term_param_clauses(ParamOwner::Given)
                    .into_iter()
                    .filter(|clause| !clause.is_empty())
                    .collect();
                self.expect_arrow();
                clauses
            } else {
                Vec::new()
            };

        let tpt = self.with_location(location, |parser| {
            parser.with_parse_kind(ParseKind::Type, |parser| parser.simple_type())
        });

        if self.current_is_given_colon() {
            return self.parse_structural_given(
                mark,
                name,
                type_params,
                value_param_clauses,
                tpt,
                prefix.metadata,
            );
        }

        let mut metadata = prefix.metadata;
        if !metadata.modifiers.contains(&Modifier::Given) {
            metadata.modifiers.push(Modifier::Given);
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
                    method_like,
                    tpt,
                    rhs,
                    metadata,
                },
            );
        }

        self.advance();
        if !method_like
            && !metadata.modifiers.contains(&Modifier::Inline)
            && !metadata.modifiers.contains(&Modifier::Erased)
        {
            metadata.modifiers.push(Modifier::Final);
            metadata.modifiers.push(Modifier::Lazy);
        }
        let rhs = self.with_location(location, |parser| parser.expr());
        self.alloc_given_definition(
            mark,
            GivenSignature {
                name,
                type_params,
                value_param_clauses,
                method_like,
                tpt,
                rhs,
                metadata,
            },
        )
    }

    fn parse_structural_given(
        &mut self,
        mark: crate::Mark,
        name: dotty_core::TermName,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        parent: TreeId<Untyped>,
        mut metadata: dotty_core::ast::Modifiers,
    ) -> ParsedStatement {
        if !metadata.modifiers.contains(&Modifier::Given) {
            metadata.modifiers.push(Modifier::Given);
        }

        let body = self.parse_optional_template_body().members;
        let template = self.allocate_given_template(
            mark.start(),
            type_params.clone(),
            value_param_clauses.clone(),
            parent,
            body,
        );

        if type_params.is_empty() && value_param_clauses.is_empty() {
            ParsedStatement::Definition(self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(ModuleDef {
                    name,
                    template,
                    metadata,
                })),
            ))
        } else {
            let name = if self.names.resolve(name.as_name().text()).is_empty() {
                anonymous_type_name(self.names)
            } else {
                dotty_core::TypeName::new(name.as_name().text())
            };
            ParsedStatement::Definition(self.alloc_from(
                mark,
                TreeKind::TypeDef(TypeDef {
                    name,
                    rhs: template,
                    metadata,
                    variance: None,
                }),
            ))
        }
    }

    fn alloc_given_definition(
        &mut self,
        mark: crate::Mark,
        signature: GivenSignature,
    ) -> ParsedStatement {
        if !signature.method_like {
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
        let has_colon = matches!(
            next.kind,
            TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::ColonOp
                | TokenKind::Punctuation(Punctuation::Colon)
        ) && self.token_text(&next).ok() == Some(":");
        has_colon
            && matches!(
                self.cursor.lookahead(2).kind,
                TokenKind::Identifier | TokenKind::BackquotedIdentifier
            )
    }

    fn current_is_given_colon(&mut self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::ColonFollow
                | TokenKind::ColonEol
                | TokenKind::ColonOp
                | TokenKind::Punctuation(Punctuation::Colon)
        ) && self.current_text_is(":")
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
    use dotty_core::{NameInterner, Punctuation, TextRange};

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

    #[test]
    fn parses_a_named_given_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given config: Config = makeConfig",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Punctuation(Punctuation::Colon), 12, 13),
                token(TokenKind::Identifier, 14, 20),
                token(TokenKind::Operator, 21, 22),
                token(TokenKind::Identifier, 23, 33),
                token(TokenKind::Eof, 33, 33),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a given definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a value definition");
        };
        let name_id = definition.name.as_name().text();
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(_)
        ));
        assert!(definition.rhs.is_some());
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "config");
    }

    #[test]
    fn parses_a_parameterized_given_alias_as_a_method_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given [A] => Show = makeShow",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method definition");
        };
        assert_eq!(definition.type_params.len(), 1);
        assert!(definition.value_param_clauses.is_empty());
        assert!(definition.metadata.modifiers.contains(&Modifier::Given));
        assert!(!definition.metadata.modifiers.contains(&Modifier::Final));
        assert!(!definition.metadata.modifiers.contains(&Modifier::Lazy));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_named_using_parameters_method_like_on_a_given_alias() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given (using ctx: Ctx) => Service = makeService",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Identifier, 13, 16),
                token(TokenKind::Punctuation(Punctuation::Colon), 16, 17),
                token(TokenKind::Identifier, 18, 21),
                token(TokenKind::Punctuation(Punctuation::RightParen), 21, 22),
                token(TokenKind::Operator, 23, 25),
                token(TokenKind::Identifier, 26, 33),
                token(TokenKind::Operator, 34, 35),
                token(TokenKind::Identifier, 36, 46),
                token(TokenKind::Eof, 46, 46),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method definition");
        };
        let [parameter] = definition.value_param_clauses[0].as_slice() else {
            panic!("expected one using parameter");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter).kind else {
            panic!("expected a value parameter");
        };
        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(
            !parameter
                .metadata
                .modifiers
                .contains(&Modifier::ParamAccessor)
        );
        assert!(
            !parameter
                .metadata
                .modifiers
                .contains(&Modifier::PrivateLocal)
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_inline_prefix_without_synthesizing_lazy() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "inline given Config = makeConfig",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Keyword(HardKeyword::Given), 7, 12),
                token(TokenKind::Identifier, 13, 19),
                token(TokenKind::Operator, 20, 21),
                token(TokenKind::Identifier, 22, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method definition");
        };
        assert!(definition.metadata.modifiers.contains(&Modifier::Inline));
        assert!(definition.metadata.modifiers.contains(&Modifier::Given));
        assert!(!definition.metadata.modifiers.contains(&Modifier::Lazy));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_private_prefix_on_a_named_given() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "private given config: Config = makeConfig",
            vec![
                token(TokenKind::Keyword(HardKeyword::Private), 0, 7),
                token(TokenKind::Keyword(HardKeyword::Given), 8, 13),
                token(TokenKind::Identifier, 14, 20),
                token(TokenKind::Punctuation(Punctuation::Colon), 20, 21),
                token(TokenKind::Identifier, 22, 28),
                token(TokenKind::Operator, 29, 30),
                token(TokenKind::Identifier, 31, 41),
                token(TokenKind::Eof, 41, 41),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a given definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a value definition");
        };
        assert!(matches!(
            definition.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_indented_structural_given_through_template_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given Service:\n  def run = result",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 13),
                token(TokenKind::ColonEol, 13, 14),
                token(TokenKind::Indent, 15, 15),
                token(TokenKind::Keyword(HardKeyword::Def), 17, 20),
                token(TokenKind::Identifier, 21, 24),
                token(TokenKind::Operator, 25, 26),
                token(TokenKind::Identifier, 27, 33),
                token(TokenKind::Outdent, 33, 33),
                token(TokenKind::Eof, 33, 33),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a structural given");
        };
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::ModuleDef(given)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a module definition");
        };
        let TreeKind::Template(template) = &parser.ast().get(given.template).kind else {
            panic!("expected a template");
        };
        assert_eq!(template.parents.len(), 1);
        assert_eq!(template.body.len(), 1);
        assert!(matches!(
            parser.ast().get(template.body[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_an_explicit_empty_given_clause_as_a_method_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given () => Empty = makeEmpty",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 29),
                token(TokenKind::Eof, 29, 29),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method definition");
        };
        assert!(definition.value_param_clauses.is_empty());
        assert!(!definition.metadata.modifiers.contains(&Modifier::Final));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_given_signature_without_its_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given [A] Show",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Identifier, 10, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let _ = parser.parse_statement(Location::Elsewhere);
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }
}
