//! Source-level Scala 3.9 contextual `given` definitions.
//!
//! Alias, conditional, and structural forms share the same parser dispatch;
//! semantic given synthesis remains outside this source-parser layer.

use dotty_core::ast::{DefDef, Modifier, Modifiers, ModuleDef, New, TypeDef, ValDef};
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
    rhs: Option<TreeId<Untyped>>,
    metadata: dotty_core::ast::Modifiers,
}

struct GivenStructure {
    type_params: Vec<TreeId<Untyped>>,
    value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
    parents: Vec<TreeId<Untyped>>,
    has_with_template_body: bool,
    template_body_feedback: Option<u32>,
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

        let has_parameterized_name = self.starts_parameterized_named_given();
        let has_name = has_parameterized_name || self.starts_named_given();
        let name = if has_name {
            let name = self
                .intern_current_term_name()
                .unwrap_or_else(|_| anonymous_term_name(self.names));
            self.advance();
            if !has_parameterized_name {
                self.advance(); // `:`
            }
            name
        } else {
            anonymous_term_name(self.names)
        };

        let type_params = if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket)
        {
            let params = self.parse_type_param_clause(ParamOwner::Given);
            if self.current_is_arrow() {
                self.advance();
            }
            params
        } else {
            Vec::new()
        };
        let mut value_param_clauses = Vec::new();
        let mut num_lead_params = 0;
        let mut has_explicit_parameter_clause = false;
        if has_parameterized_name {
            self.parse_given_parameter_clauses(
                &mut value_param_clauses,
                &mut num_lead_params,
                &mut has_explicit_parameter_clause,
                true,
            );
            self.consume_newlines_before_given_colon();
            if self.current_is_given_colon() {
                self.advance();
            } else {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected `:` after a named given signature",
                );
            }
        } else if !type_params.is_empty() && self.current_is_given_colon() {
            // Anonymous parameterized givens may use the legacy colon between
            // their type-parameter clause and the implemented given type.
            self.advance();
        }
        let (parents, has_with_template_body, template_body_feedback) = loop {
            if self.starts_given_parameter_clause() {
                self.parse_given_parameter_clauses(
                    &mut value_param_clauses,
                    &mut num_lead_params,
                    &mut has_explicit_parameter_clause,
                    false,
                );
                continue;
            }

            let mark = self.mark();
            let candidate = self.parse_given_parent(location);
            let is_given_type = !matches!(self.ast.get(candidate).kind, TreeKind::Apply(_));
            if self.current_is_arrow() && is_given_type {
                self.advance();
                let parameter_position = self.ast.get(candidate).position;
                let metadata = Modifiers {
                    modifiers: vec![Modifier::Given, Modifier::Param],
                    ..Modifiers::default()
                };
                let parameter = self.alloc_synthetic_context_parameter(
                    mark,
                    candidate,
                    num_lead_params.saturating_add(1),
                    metadata,
                );
                self.ast.get_mut(parameter).position = parameter_position;
                num_lead_params = num_lead_params.saturating_add(1);
                value_param_clauses.push(vec![parameter]);
                continue;
            }
            let mut parents = vec![candidate];
            let (has_with_template_body, template_body_feedback) =
                self.parse_given_parent_suffixes(&mut parents, location);
            break (parents, has_with_template_body, template_body_feedback);
        };
        let method_like = !type_params.is_empty()
            || has_explicit_parameter_clause
            || !value_param_clauses.is_empty()
            || prefix
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Inline)
            || prefix
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Erased);

        let structural = self.current_is_given_colon()
            || has_with_template_body
            || parents.len() > 1
            || parents
                .iter()
                .any(|parent| matches!(self.ast.get(*parent).kind, TreeKind::Apply(_)));
        if structural {
            return self.parse_structural_given(
                mark,
                name,
                GivenStructure {
                    type_params,
                    value_param_clauses,
                    parents,
                    has_with_template_body,
                    template_body_feedback,
                },
                prefix.metadata,
            );
        }

        let tpt = parents[0];

        let mut metadata = prefix.metadata;
        if !metadata.modifiers.contains(&Modifier::Given) {
            metadata.modifiers.push(Modifier::Given);
        }

        if !self.current_is_bare_assignment() {
            if has_name && is_abstract_named_given_boundary(self.current().kind) {
                return self.alloc_given_definition(
                    mark,
                    GivenSignature {
                        name,
                        type_params,
                        value_param_clauses,
                        method_like: true,
                        tpt,
                        rhs: None,
                        metadata,
                    },
                );
            }
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
                    rhs: Some(rhs),
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
                rhs: Some(rhs),
                metadata,
            },
        )
    }

    fn parse_structural_given(
        &mut self,
        mark: crate::Mark,
        name: dotty_core::TermName,
        structure: GivenStructure,
        mut metadata: dotty_core::ast::Modifiers,
    ) -> ParsedStatement {
        if !metadata.modifiers.contains(&Modifier::Given) {
            metadata.modifiers.push(Modifier::Given);
        }

        let body = self
            .with_secondary_constructor_allowed(false, |parser| {
                parser.with_enum_body(false, |parser| {
                    parser.parse_optional_template_body_with_feedback(
                        structure.template_body_feedback,
                        structure.has_with_template_body,
                    )
                })
            })
            .members;
        let template = self.allocate_given_template(
            mark.start(),
            structure.type_params.clone(),
            structure.value_param_clauses.clone(),
            structure.parents,
            body,
        );

        if structure.type_params.is_empty() && structure.value_param_clauses.is_empty() {
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

    fn parse_given_parent_suffixes(
        &mut self,
        parents: &mut Vec<TreeId<Untyped>>,
        location: Location,
    ) -> (bool, Option<u32>) {
        loop {
            let newline_count = self.newlines_before_given_parent_separator();
            let separator = matches!(
                self.cursor.lookahead(newline_count).kind,
                TokenKind::Punctuation(Punctuation::Comma) | TokenKind::Keyword(HardKeyword::With)
            );
            if !separator {
                return (false, None);
            }
            let is_with =
                self.cursor.lookahead(newline_count).kind == TokenKind::Keyword(HardKeyword::With);
            for _ in 0..newline_count {
                self.advance();
            }
            let with_template_body = is_with && self.given_with_starts_template_body();
            let template_body_feedback =
                if with_template_body && self.given_with_ends_physical_line() {
                    self.observe_indented_body_region()
                } else {
                    None
                };
            self.advance();
            if with_template_body {
                return (true, template_body_feedback);
            }
            if matches!(
                self.current().kind,
                TokenKind::ColonFollow
                    | TokenKind::ColonEol
                    | TokenKind::ColonOp
                    | TokenKind::Punctuation(Punctuation::Colon)
                    | TokenKind::Eof
                    | TokenKind::Indent
                    | TokenKind::Outdent
            ) && self.current_text().ok() == Some(":")
            {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a given parent after the separator",
                );
                return (false, None);
            }
            parents.push(self.parse_given_parent(location));
        }
    }

    /// In Scala 3.9, `withConstrApps` accepts a parent only when it follows
    /// `with` on the same line. A line end or `{` after `with` starts the
    /// legacy `with` template body instead.
    fn given_with_starts_template_body(&mut self) -> bool {
        self.current().kind == TokenKind::Keyword(HardKeyword::With)
            && (matches!(
                self.cursor.lookahead(1).kind,
                TokenKind::Indent | TokenKind::Punctuation(Punctuation::LeftBrace)
            ) || self.given_with_ends_physical_line())
    }

    fn given_with_ends_physical_line(&mut self) -> bool {
        let with_end = self.current().span.end();
        let mut lookahead = 1;
        while matches!(
            self.cursor.lookahead(lookahead).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            lookahead += 1;
        }
        let next_start = self.cursor.lookahead(lookahead).span.start();
        self.has_line_break_between(with_end, next_start)
    }

    fn parse_given_parent(&mut self, location: Location) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut parent = self.with_parse_kind(ParseKind::Type, |parser| {
            if parser.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
                parser.with_location(location, |parser| parser.type_expr())
            } else {
                parser.with_location(location, |parser| parser.parse_infix_type())
            }
        });
        if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket) {
            parent = self.parse_type_application(mark, parent);
        }
        if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
            let new_tree = self.alloc(
                TreeKind::New(New { tpt: parent }),
                self.ast.get(parent).position,
            );
            parent = self.parse_application(mark, new_tree);
            while self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
                parent = self.parse_application(mark, parent);
            }
        }
        parent
    }

    fn newlines_before_given_parent_separator(&mut self) -> usize {
        let mut count = 0;
        while matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            count += 1;
        }
        if matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Punctuation(Punctuation::Comma) | TokenKind::Keyword(HardKeyword::With)
        ) {
            count
        } else {
            0
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
                    rhs: signature.rhs,
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
                    source_param_clause_order: None,
                    tpt: signature.tpt,
                    rhs: signature.rhs,
                    metadata: signature.metadata,
                }),
            ))
        }
    }

    fn starts_given_parameter_clause(&mut self) -> bool {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return false;
        }

        let mut depth = 0usize;
        let mut offset = 0usize;
        loop {
            match self.cursor.lookahead(offset).kind {
                TokenKind::Punctuation(Punctuation::LeftParen) => depth += 1,
                TokenKind::Punctuation(Punctuation::RightParen) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self.lookahead_is_arrow(offset + 1);
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset += 1;
        }
    }

    fn parse_given_parameter_clauses(
        &mut self,
        value_param_clauses: &mut Vec<Vec<TreeId<Untyped>>>,
        num_lead_params: &mut usize,
        has_explicit_parameter_clause: &mut bool,
        allow_without_arrow: bool,
    ) {
        loop {
            if allow_without_arrow {
                self.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
                    Punctuation::LeftParen,
                ));
            }
            let is_parameter_clause = self.starts_given_parameter_clause()
                || (allow_without_arrow
                    && self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen));
            if !is_parameter_clause {
                break;
            }

            let clause_mark = self.mark();
            *has_explicit_parameter_clause = true;
            let is_using = self.current_is_using_parameter_clause();
            let clause = self.parse_single_term_param_clause(
                ParamOwner::Given,
                true,
                is_using,
                *num_lead_params,
            );
            *num_lead_params += clause.len();
            if !clause.is_empty() {
                value_param_clauses.push(clause);
            } else if *num_lead_params > 0 {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a context parameter after `()`",
                );
                let missing_type = self.error_type(self.zero_width_span(clause_mark.start()));
                let parameter = self.alloc_synthetic_context_parameter(
                    clause_mark,
                    missing_type,
                    num_lead_params.saturating_add(1),
                    Modifiers {
                        modifiers: vec![Modifier::Given, Modifier::Param],
                        ..Modifiers::default()
                    },
                );
                self.ast.get_mut(parameter).position =
                    Some(self.zero_width_span(clause_mark.start()));
                *num_lead_params = num_lead_params.saturating_add(1);
                value_param_clauses.push(vec![parameter]);
            }

            if self.current_is_arrow() {
                self.advance();
            } else if !allow_without_arrow {
                self.expect_arrow();
            }
        }
    }

    fn consume_newlines_before_given_colon(&mut self) {
        let mut newline_count = 0;
        while matches!(
            self.cursor.lookahead(newline_count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            newline_count += 1;
        }
        if self.cursor.lookahead(newline_count).kind == TokenKind::ColonEol
            || self.cursor.lookahead(newline_count).kind == TokenKind::ColonFollow
            || self.cursor.lookahead(newline_count).kind == TokenKind::ColonOp
            || self.cursor.lookahead(newline_count).kind
                == TokenKind::Punctuation(Punctuation::Colon)
        {
            for _ in 0..newline_count {
                self.advance();
            }
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
        let candidate = self.cursor.lookahead(2).clone();
        has_colon
            && matches!(
                candidate.kind,
                TokenKind::Identifier
                    | TokenKind::BackquotedIdentifier
                    | TokenKind::Punctuation(Punctuation::LeftBracket)
                    | TokenKind::Punctuation(Punctuation::LeftParen)
            )
            && !self.has_line_break_between(next.span.end(), candidate.span.start())
    }

    fn starts_parameterized_named_given(&mut self) -> bool {
        if !matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return false;
        }

        let mut offset = 1;
        let mut saw_parameter_clause = false;
        loop {
            while matches!(
                self.cursor.lookahead(offset).kind,
                TokenKind::Newline | TokenKind::Newlines
            ) {
                offset += 1;
            }
            let opening = self.cursor.lookahead(offset).kind;
            let closing = match opening {
                TokenKind::Punctuation(Punctuation::LeftParen) => {
                    TokenKind::Punctuation(Punctuation::RightParen)
                }
                TokenKind::Punctuation(Punctuation::LeftBracket) => {
                    TokenKind::Punctuation(Punctuation::RightBracket)
                }
                _ => break,
            };

            let mut delimiters = vec![closing];
            offset += 1;
            while !delimiters.is_empty() {
                let kind = self.cursor.lookahead(offset).kind;
                let nested_closing = match kind {
                    TokenKind::Punctuation(Punctuation::LeftParen) => {
                        Some(TokenKind::Punctuation(Punctuation::RightParen))
                    }
                    TokenKind::Punctuation(Punctuation::LeftBracket) => {
                        Some(TokenKind::Punctuation(Punctuation::RightBracket))
                    }
                    _ => None,
                };
                if let Some(nested_closing) = nested_closing {
                    delimiters.push(nested_closing);
                } else if matches!(
                    kind,
                    TokenKind::Punctuation(Punctuation::RightParen)
                        | TokenKind::Punctuation(Punctuation::RightBracket)
                ) {
                    if delimiters.pop() != Some(kind) {
                        return false;
                    }
                } else if kind == TokenKind::Eof {
                    return false;
                }
                offset += 1;
            }
            saw_parameter_clause = true;
        }

        let colon = self.cursor.lookahead(offset).clone();
        let candidate = self.cursor.lookahead(offset.saturating_add(1)).clone();
        saw_parameter_clause
            && matches!(
                colon.kind,
                TokenKind::ColonFollow
                    | TokenKind::ColonEol
                    | TokenKind::ColonOp
                    | TokenKind::Punctuation(Punctuation::Colon)
            )
            && self.token_text(&colon).ok() == Some(":")
            && !self.has_line_break_between(colon.span.end(), candidate.span.start())
    }

    fn has_line_break_between(&self, start: u32, end: u32) -> bool {
        self.source
            .as_str()
            .get(start as usize..end as usize)
            .map(|text| text.chars().any(dotty_core::is_line_break_char))
            .unwrap_or(true)
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

fn is_abstract_named_given_boundary(kind: TokenKind) -> bool {
    crate::definitions::is_definition_boundary(kind)
        || kind == TokenKind::Punctuation(Punctuation::RightParen)
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
    fn preserves_and_diagnoses_repeated_parameters_in_a_named_given() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given c: (xs: A*) => C = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::Colon), 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 12),
                token(TokenKind::ColonFollow, 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Operator, 18, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Operator, 23, 24),
                token(TokenKind::Identifier, 25, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected a named given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method-like given definition");
        };
        let TreeKind::ValDef(parameter) =
            &parser.ast().get(definition.value_param_clauses[0][0]).kind
        else {
            panic!("expected a given parameter ValDef");
        };

        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PostfixOp(_))
        ));
        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken
                && diagnostic
                    .message()
                    .contains("not allowed in `given` or `implicit`")
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_named_given_alias_with_a_parenthesized_function_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given f: (A => B) = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::Colon), 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Operator, 12, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 25),
                token(TokenKind::Eof, 25, 25),
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
            parser.ast().get(definition.tpt).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Parens(_))
        ));
        assert!(definition.rhs.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_abstract_named_given_at_a_statement_boundary() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given TreeMethods: TreeMethods",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 17),
                token(TokenKind::Punctuation(Punctuation::Colon), 17, 18),
                token(TokenKind::Identifier, 19, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected an abstract given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected an abstract given method definition");
        };
        let name_id = definition.name.as_name().text();
        assert!(definition.rhs.is_none());
        assert!(definition.metadata.modifiers.contains(&Modifier::Given));
        assert!(!definition.metadata.modifiers.contains(&Modifier::Final));
        assert!(!definition.metadata.modifiers.contains(&Modifier::Lazy));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 30).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "TreeMethods");
    }

    #[test]
    fn still_reports_a_missing_equals_before_a_non_boundary_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given config: Config]",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Punctuation(Punctuation::Colon), 12, 13),
                token(TokenKind::Identifier, 14, 20),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected a recoverable given definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("invalid alias syntax should retain its recovery tree");
        };
        assert!(definition.rhs.is_some());
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(parser.diagnostics()[0].message().contains("expected `=`"));
    }

    #[test]
    fn accepts_an_abstract_named_given_before_a_closing_parenthesis() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given config: Config)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 12),
                token(TokenKind::Punctuation(Punctuation::Colon), 12, 13),
                token(TokenKind::Identifier, 14, 20),
                token(TokenKind::Punctuation(Punctuation::RightParen), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected an abstract given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected an abstract given method definition");
        };
        assert!(definition.rhs.is_none());
        assert_eq!(
            parser.current().kind,
            TokenKind::Punctuation(Punctuation::RightParen)
        );
        assert!(parser.diagnostics().is_empty());
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
    fn parses_a_parameterized_given_alias_without_an_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given [A] Show = makeShow",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Identifier, 10, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Identifier, 17, 25),
                token(TokenKind::Eof, 25, 25),
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
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_anonymous_parameterized_given_with_colon_separator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given [A]: Show = makeShow",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Punctuation(Punctuation::Colon), 9, 10),
                token(TokenKind::Identifier, 11, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::Identifier, 18, 26),
                token(TokenKind::Eof, 26, 26),
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
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_named_generic_given_with_a_using_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given foo[T](using ctx: Ctx): Out[T] = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 12, 13),
                token(TokenKind::Identifier, 13, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Punctuation(Punctuation::Colon), 22, 23),
                token(TokenKind::Identifier, 24, 27),
                token(TokenKind::Punctuation(Punctuation::RightParen), 27, 28),
                token(TokenKind::Punctuation(Punctuation::Colon), 28, 29),
                token(TokenKind::Identifier, 30, 33),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 33, 34),
                token(TokenKind::Identifier, 34, 35),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 35, 36),
                token(TokenKind::Operator, 37, 38),
                token(TokenKind::Identifier, 39, 44),
                token(TokenKind::Eof, 44, 44),
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
            panic!("expected a parameter value definition");
        };
        assert_eq!(
            parser.names.resolve(definition.name.as_name().text()),
            "foo"
        );
        assert_eq!(definition.type_params.len(), 1);
        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(definition.rhs.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_named_generic_given_with_a_this_type_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given self[T]: this.type = this",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 10),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 12, 13),
                token(TokenKind::Punctuation(Punctuation::Colon), 13, 14),
                token(TokenKind::Keyword(HardKeyword::This), 15, 19),
                token(TokenKind::Punctuation(Punctuation::Dot), 19, 20),
                token(TokenKind::Keyword(HardKeyword::Type), 20, 24),
                token(TokenKind::Operator, 25, 26),
                token(TokenKind::Keyword(HardKeyword::This), 27, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a named generic given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method-like given definition");
        };
        assert_eq!(
            parser.names.resolve(definition.name.as_name().text()),
            "self"
        );
        assert_eq!(definition.type_params.len(), 1);
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::SingletonTypeTree(_)
        ));
        assert!(definition.rhs.is_some());
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
    fn parses_an_anonymous_given_type_condition_as_a_synthetic_using_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given H => I = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Operator, 13, 14),
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method-like given definition");
        };
        let [parameter] = definition.value_param_clauses[0].as_slice() else {
            panic!("expected one synthetic context parameter");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter).kind else {
            panic!("expected a value parameter");
        };
        assert_eq!(parser.names.resolve(parameter.name.as_name().text()), "x$1");
        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn chains_anonymous_given_type_conditions_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given H => I => J = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method-like given definition");
        };
        assert_eq!(definition.value_param_clauses.len(), 2);
        let parameter_names = definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                let TreeKind::ValDef(parameter) = &parser.ast().get(clause[0]).kind else {
                    panic!("expected a synthetic context parameter");
                };
                parser.names.resolve(parameter.name.as_name().text())
            })
            .collect::<Vec<_>>();
        assert_eq!(parameter_names, ["x$1", "x$2"]);
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_parenthesized_anonymous_given_type_conditions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given (H, I) => J = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Operator, 18, 19),
                token(TokenKind::Identifier, 20, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method-like given definition");
        };
        assert_eq!(definition.value_param_clauses.len(), 1);
        assert_eq!(definition.value_param_clauses[0].len(), 2);
        let names = definition.value_param_clauses[0]
            .iter()
            .map(|parameter| {
                let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter).kind else {
                    panic!("expected a value parameter");
                };
                parser.names.resolve(parameter.name.as_name().text())
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["x$1", "x$2"]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn marks_ordinary_conditional_given_parameters_as_contextual() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given (ctx: Ctx) => Service = makeService",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::Colon), 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Operator, 17, 19),
                token(TokenKind::Identifier, 20, 27),
                token(TokenKind::Operator, 28, 29),
                token(TokenKind::Identifier, 30, 40),
                token(TokenKind::Eof, 40, 40),
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
            panic!("expected one conditional parameter");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter).kind else {
            panic!("expected a value parameter");
        };
        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
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
    fn parses_a_structural_given_with_with_separated_parents() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A with B",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::With), 8, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Eof, 14, 14),
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
        assert_eq!(template.parents.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_given_with_body_after_a_line_final_with() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A with\n  def run = result",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::With), 8, 12),
                token(TokenKind::Newline, 12, 13),
                token(TokenKind::Indent, 15, 15),
                token(TokenKind::Keyword(HardKeyword::Def), 15, 18),
                token(TokenKind::Identifier, 19, 22),
                token(TokenKind::Operator, 23, 24),
                token(TokenKind::Identifier, 25, 31),
                token(TokenKind::Outdent, 31, 31),
                token(TokenKind::Eof, 31, 31),
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
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_explicit_with_parent_before_a_line_final_body_with() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A with B with\n  def run = result",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::With), 8, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Keyword(HardKeyword::With), 15, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Indent, 22, 22),
                token(TokenKind::Keyword(HardKeyword::Def), 22, 25),
                token(TokenKind::Identifier, 26, 29),
                token(TokenKind::Operator, 30, 31),
                token(TokenKind::Identifier, 32, 38),
                token(TokenKind::Outdent, 38, 38),
                token(TokenKind::Eof, 38, 38),
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
        assert_eq!(template.parents.len(), 2);
        assert_eq!(template.body.len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_missing_with_body_without_consuming_the_next_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A with\nval next = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::With), 8, 12),
                token(TokenKind::Newline, 12, 13),
                token(TokenKind::Keyword(HardKeyword::Val), 13, 16),
                token(TokenKind::Identifier, 17, 21),
                token(TokenKind::Operator, 22, 23),
                token(TokenKind::IntegerLiteral, 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let _ = parser.parse_given_definition(Location::Elsewhere);

        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].message(),
            "expected a template body after `with`"
        );
        assert_eq!(parser.current().kind, TokenKind::Newline);
        parser.advance();
        assert_eq!(parser.current().kind, TokenKind::Keyword(HardKeyword::Val));
    }

    #[test]
    fn parses_a_structural_given_with_constructor_and_type_parents() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given C() with D",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Keyword(HardKeyword::With), 10, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Eof, 16, 16),
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
        assert_eq!(template.parents.len(), 2);
        assert!(matches!(
            parser.ast().get(template.parents[0]).kind,
            TreeKind::Apply(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn treats_a_constructor_parent_before_an_arrow_as_structural() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A() => B = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Operator, 10, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Identifier, 17, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
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
        assert!(matches!(
            parser.ast().get(template.parents[0]).kind,
            TreeKind::Apply(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_structural_given_with_comma_separated_parents() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given C(), D",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Eof, 12, 12),
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
        assert_eq!(template.parents.len(), 2);
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
    fn reports_a_missing_equals_after_a_parameterized_given_type() {
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
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message()
                .contains("expected `=` after a given type")
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_from_a_generic_named_given_without_its_type_before_the_next_member() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given foo[T]: Out[T] ? def next = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Punctuation(Punctuation::Colon), 12, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 17, 18),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 19, 20),
                token(TokenKind::Operator, 21, 22),
                token(TokenKind::Keyword(HardKeyword::Def), 23, 26),
                token(TokenKind::Identifier, 27, 31),
                token(TokenKind::Operator, 32, 33),
                token(TokenKind::Identifier, 34, 39),
                token(TokenKind::Eof, 39, 39),
            ],
            &mut names,
        );

        let _ = parser.parse_statement(Location::Elsewhere);
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Operator);

        parser.advance();
        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected the following method to remain parseable");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected the following method definition");
        };
        assert_eq!(
            parser.names.resolve(definition.name.as_name().text()),
            "next"
        );
    }

    #[test]
    fn recovers_from_a_given_type_condition_without_swallowing_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given H =>",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let _ = parser.parse_given_definition(Location::Elsewhere);

        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_a_later_empty_given_clause_as_an_error_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A => () => B = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Operator, 14, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::Identifier, 21, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_given_definition(Location::Elsewhere)
        else {
            panic!("expected a given definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a method-like given definition");
        };
        assert_eq!(definition.value_param_clauses.len(), 2);
        assert_eq!(definition.value_param_clauses[0].len(), 1);
        assert_eq!(definition.value_param_clauses[1].len(), 1);
        let TreeKind::ValDef(parameter) =
            &parser.ast().get(definition.value_param_clauses[1][0]).kind
        else {
            panic!("expected a recovered context parameter");
        };
        assert_eq!(parser.names.resolve(parameter.name.as_name().text()), "x$2");
        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::Error(
                dotty_core::ast::ErrorNode {
                    kind: dotty_core::ast::ErrorNodeKind::MissingType
                }
            ))
        ));
        assert_eq!(
            parser
                .ast()
                .get(definition.value_param_clauses[1][0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(11, 11).unwrap()
        );
        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn recovers_from_a_missing_given_parent_after_with() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given A with",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Keyword(HardKeyword::With), 8, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let _ = parser.parse_given_definition(Location::Elsewhere);

        assert!(!parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn does_not_treat_a_multiline_colon_as_a_named_given() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "given named:\nvalue",
            vec![
                token(TokenKind::Keyword(HardKeyword::Given), 0, 5),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::ColonEol, 11, 12),
                token(TokenKind::Identifier, 13, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        parser.advance();
        assert!(!parser.starts_named_given());
    }
}
