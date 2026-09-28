//! Reusable source-level term parameter clauses.
//!
//! Parameters are represented by the existing [`dotty_core::ast::ValDef`]
//! node. This module owns the clause/parameter boundary so definitions,
//! constructors, givens, and extensions can reuse it without duplicating the
//! delimiter and recovery logic.

use dotty_core::ast::{ByNameTypeTree, Modifier, Modifiers, ValDef};
use dotty_core::{Punctuation, SourceSpan, TermName, TokenKind, TreeId, TreeKind, Untyped};

use crate::names::synthetic_term_param_name;
use crate::{ParamOwner, ParseDiagnosticKind, ParseKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses all consecutive ordinary term parameter clauses.
    pub(crate) fn parse_term_param_clauses(
        &mut self,
        owner: ParamOwner,
    ) -> Vec<Vec<TreeId<Untyped>>> {
        self.with_param_owner(Some(owner), |parser| {
            let mut clauses = Vec::new();
            let mut first_ordinary_clause = true;
            let mut num_lead_params = 0;
            loop {
                parser.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
                    Punctuation::LeftParen,
                ));
                if parser.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
                    break;
                }
                let is_implicit = parser.current_is_implicit_parameter_clause();
                let is_using = parser.current_is_using_parameter_clause();
                let clause = if is_using {
                    parser.parse_term_param_clause_with_policy(
                        owner,
                        false,
                        true,
                        false,
                        num_lead_params,
                    )
                } else {
                    let clause = parser.parse_term_param_clause_with_policy(
                        owner,
                        first_ordinary_clause,
                        false,
                        is_implicit,
                        num_lead_params,
                    );
                    first_ordinary_clause = false;
                    clause
                };
                num_lead_params += clause.len();
                clauses.push(clause);
                if is_implicit {
                    break;
                }
            }
            clauses
        })
    }

    /// Parses one term parameter clause for a grammar production that needs
    /// to enforce its own clause ordering, such as `extension`.
    pub(crate) fn parse_single_term_param_clause(
        &mut self,
        owner: ParamOwner,
        first_ordinary_clause: bool,
        is_using: bool,
        num_lead_params: usize,
    ) -> Vec<TreeId<Untyped>> {
        let is_implicit = self.current_is_implicit_parameter_clause();
        self.with_param_owner(Some(owner), |parser| {
            parser.parse_term_param_clause_with_policy(
                owner,
                first_ordinary_clause,
                is_using,
                is_implicit,
                num_lead_params,
            )
        })
    }

    /// Consumes layout separators only when they lead to the requested
    /// parameter-clause delimiter. A newline before a return type or method
    /// body remains available to the enclosing grammar.
    pub(crate) fn consume_newlines_before_parameter_clause(&mut self, expected: TokenKind) {
        let mut newline_count = 0;
        while matches!(
            self.cursor.lookahead(newline_count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            newline_count += 1;
        }
        if self.cursor.lookahead(newline_count).kind != expected {
            return;
        }
        for _ in 0..newline_count {
            self.advance();
        }
    }

    fn parse_term_param_clause_with_policy(
        &mut self,
        owner: ParamOwner,
        first_ordinary_clause: bool,
        is_using: bool,
        is_implicit: bool,
        num_lead_params: usize,
    ) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftParen));
        let mut params = Vec::new();
        let mut metadata = Modifiers::default();

        if is_implicit
            && !matches!(
                owner,
                ParamOwner::Def
                    | ParamOwner::Class
                    | ParamOwner::CaseClass
                    | ParamOwner::ExtensionFollow
            )
        {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "legacy `implicit` clauses are not allowed for this parameter owner",
            );
        }

        if is_using || owner == ParamOwner::Given {
            metadata.modifiers.push(Modifier::Given);
        }

        if is_implicit {
            metadata.modifiers.push(Modifier::Implicit);
            self.advance();
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a parameter after `implicit`",
                );
                return params;
            }
        }

        if is_using {
            self.advance();
            if self.current_is_anonymous_using_type() {
                return self.parse_anonymous_using_types(
                    owner,
                    first_ordinary_clause,
                    metadata,
                    num_lead_params,
                );
            }
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a parameter after `using`",
                );
                return params;
            }
        }

        if owner == ParamOwner::Given && current_is_anonymous_context_type(self) {
            return self.parse_anonymous_using_types(
                owner,
                first_ordinary_clause,
                metadata,
                num_lead_params,
            );
        }

        if !is_using && !is_implicit && self.accept(TokenKind::Punctuation(Punctuation::RightParen))
        {
            return params;
        }

        loop {
            if self.context.enum_body
                && matches!(
                    self.current().kind,
                    TokenKind::Eof
                        | TokenKind::Newline
                        | TokenKind::Newlines
                        | TokenKind::Indent
                        | TokenKind::Outdent
                        | TokenKind::Keyword(dotty_core::HardKeyword::Case)
                        | TokenKind::Punctuation(Punctuation::RightBrace)
                )
            {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a parameter before the end of the clause",
                );
                break;
            }
            let checkpoint = self.cursor.checkpoint();
            params.push(self.parse_term_param(owner, first_ordinary_clause, metadata.clone()));

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a term parameter",
                );
                self.recover_term_param_clause();
                break;
            }

            if self.current().kind == TokenKind::Punctuation(Punctuation::Comma) {
                let comma_end = self.current().span.end();
                self.advance();
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    if !self.comma_is_followed_by_line_break_before_right_paren(comma_end) {
                        self.report(
                            ParseDiagnosticKind::ExpectedToken,
                            "expected a parameter after `,`",
                        );
                    }
                    self.advance();
                    break;
                }
                continue;
            }

            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            break;
        }

        params
    }

    fn parse_term_param(
        &mut self,
        owner: ParamOwner,
        first_ordinary_clause: bool,
        mut metadata: Modifiers,
    ) -> TreeId<Untyped> {
        let mark = self.mark();
        while self.current().kind == TokenKind::Operator && self.current_text_is("@") {
            metadata.annotations.push(self.parse_annotation());
        }

        if is_class_parameter_owner(owner) {
            self.parse_class_parameter_modifiers(&mut metadata);
        } else if self.current_is_inline_parameter_modifier() {
            self.add_modifier(&mut metadata, Modifier::Inline);
            self.advance();
        }

        let explicit_accessor = match self.current().kind {
            TokenKind::Keyword(dotty_core::HardKeyword::Val) => {
                self.advance();
                Some(false)
            }
            TokenKind::Keyword(dotty_core::HardKeyword::Var) => {
                self.advance();
                Some(true)
            }
            _ => None,
        };
        if let Some(is_var) = explicit_accessor {
            if !is_class_parameter_owner(owner) {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "`val` and `var` parameter accessors are only valid on class constructors",
                );
            } else {
                metadata.modifiers.push(Modifier::ParamAccessor);
                if is_var {
                    metadata.modifiers.push(Modifier::Var);
                }
            }
        } else if is_class_parameter_owner(owner) {
            if metadata.visibility.is_some()
                || metadata
                    .modifiers
                    .iter()
                    .any(|modifier| matches!(modifier, Modifier::Final | Modifier::Override))
            {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "`val` or `var` expected",
                );
            }
            if owner == ParamOwner::CaseClass && first_ordinary_clause {
                metadata.modifiers.push(Modifier::ParamAccessor);
            } else {
                metadata.modifiers.push(Modifier::ParamAccessor);
                metadata.modifiers.push(Modifier::PrivateLocal);
            }
        } else {
            metadata.modifiers.push(Modifier::Param);
        }
        let name = self.parse_param_name();
        let tpt = if is_parameter_colon(self) {
            self.advance();
            self.with_parse_kind(ParseKind::Type, |parser| {
                if parser.current_is_arrow() {
                    let mark = parser.mark();
                    if is_class_parameter_owner(owner)
                        && !metadata.modifiers.contains(&Modifier::PrivateLocal)
                    {
                        let mutable = metadata.modifiers.contains(&Modifier::Var);
                        let modifier = if mutable { "var" } else { "val" };
                        parser.report_at(
                            ParseDiagnosticKind::UnexpectedToken,
                            parser.current_span(),
                            format!("`{modifier}` parameters may not be call-by-name"),
                        );
                    }
                    parser.advance();
                    let result = parser.parse_parameter_type();
                    parser.alloc_from(mark, TreeKind::ByNameTypeTree(ByNameTypeTree { result }))
                } else {
                    parser.parse_parameter_type()
                }
            })
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected `:` and a parameter type",
            );
            self.error_type(self.current_span())
        };
        if matches!(
            &self.ast.get(tpt).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::PostfixOp(_))
        ) && metadata
            .modifiers
            .iter()
            .any(|modifier| matches!(modifier, Modifier::Given | Modifier::Implicit))
            && let Some(position) = self.ast.get(tpt).position
        {
            self.report_at(
                ParseDiagnosticKind::UnexpectedToken,
                SourceSpan::new(self.source_id, position.span()),
                "repeated parameters are not allowed in `given` or `implicit` clauses",
            );
        }
        let rhs = if is_bare_assignment(self) {
            self.advance();
            Some(self.with_location(crate::Location::InArgs, |parser| parser.expr()))
        } else {
            None
        };

        self.alloc_from(
            mark,
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs,
                metadata,
            }),
        )
    }

    fn parse_class_parameter_modifiers(&mut self, metadata: &mut Modifiers) {
        loop {
            match self.current().kind {
                TokenKind::Keyword(
                    dotty_core::HardKeyword::Private | dotty_core::HardKeyword::Protected,
                ) => self.parse_visibility(metadata),
                TokenKind::Keyword(dotty_core::HardKeyword::Override) => {
                    self.add_modifier(metadata, Modifier::Override);
                    self.advance();
                }
                TokenKind::Keyword(dotty_core::HardKeyword::Final) => {
                    self.add_modifier(metadata, Modifier::Final);
                    self.advance();
                }
                TokenKind::Identifier => {
                    if !self.current_is_inline_parameter_modifier() {
                        break;
                    }
                    self.add_modifier(metadata, Modifier::Inline);
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn current_is_inline_parameter_modifier(&mut self) -> bool {
        if self.current().kind != TokenKind::Identifier || is_parameter_colon_at(self, 1) {
            return false;
        }
        let inline = self.known_names().inline;
        self.intern_current_term_name().ok() == Some(inline)
    }

    fn comma_is_followed_by_line_break_before_right_paren(&self, comma_end: u32) -> bool {
        if self.current().kind != TokenKind::Punctuation(Punctuation::RightParen) {
            return false;
        }
        // The scanner suppresses physical newlines inside parameter parens.
        self.source
            .as_str()
            .get(comma_end as usize..self.current().span.start() as usize)
            .map(|gap| gap.chars().any(dotty_core::is_line_break_char))
            .unwrap_or(false)
    }

    pub(crate) fn current_is_using_parameter_clause(&mut self) -> bool {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return false;
        }
        let token = self.cursor.lookahead(1).clone();
        token.kind == TokenKind::Identifier
            && self
                .token_text(&token)
                .ok()
                .map(|text| text == "using")
                .unwrap_or(false)
    }

    fn current_is_implicit_parameter_clause(&mut self) -> bool {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return false;
        }
        if self.cursor.lookahead(1).kind == TokenKind::Keyword(dotty_core::HardKeyword::Implicit) {
            return true;
        }
        false
    }

    fn current_is_anonymous_using_type(&mut self) -> bool {
        self.current().kind != TokenKind::Punctuation(Punctuation::RightParen)
            && !is_parameter_colon_at(self, 1)
            && !self.current_is_inline_parameter_modifier()
    }

    fn parse_anonymous_using_types(
        &mut self,
        owner: ParamOwner,
        first_ordinary_clause: bool,
        metadata: Modifiers,
        num_lead_params: usize,
    ) -> Vec<TreeId<Untyped>> {
        let mut params = Vec::new();
        let mut next_index = num_lead_params.saturating_add(1);

        loop {
            if self.current().kind == TokenKind::Punctuation(Punctuation::RightParen) {
                if params.is_empty() {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected a parameter type after `using`",
                    );
                }
                self.advance();
                break;
            }
            if matches!(
                self.current().kind,
                TokenKind::Eof
                    | TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
            ) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a parameter type after `using`",
                );
                break;
            }

            let mark = self.mark();
            let checkpoint = self.cursor.checkpoint();
            let tpt = self.with_parse_kind(ParseKind::Type, |parser| parser.type_expr());
            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing an anonymous `using` parameter",
                );
                self.recover_term_param_clause();
                break;
            }

            let mut parameter_metadata = metadata.clone();
            add_class_parameter_metadata(owner, first_ordinary_clause, &mut parameter_metadata);
            if !is_class_parameter_owner(owner) {
                parameter_metadata.modifiers.push(Modifier::Param);
            }
            let parameter =
                self.alloc_synthetic_context_parameter(mark, tpt, next_index, parameter_metadata);
            next_index = next_index.saturating_add(1);
            params.push(parameter);

            if self.current().kind == TokenKind::Punctuation(Punctuation::Comma) {
                let comma_end = self.current().span.end();
                self.advance();
                if self.current().kind == TokenKind::Punctuation(Punctuation::RightParen) {
                    if !self.comma_is_followed_by_line_break_before_right_paren(comma_end) {
                        self.report(
                            ParseDiagnosticKind::ExpectedToken,
                            "expected a parameter type after `,`",
                        );
                    }
                    self.advance();
                    break;
                }
                continue;
            }

            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            break;
        }

        params
    }

    pub(crate) fn alloc_synthetic_context_parameter(
        &mut self,
        mark: crate::Mark,
        tpt: TreeId<Untyped>,
        index: usize,
        metadata: Modifiers,
    ) -> TreeId<Untyped> {
        let name = synthetic_term_param_name(self.names, index);
        self.alloc_from(
            mark,
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs: None,
                metadata,
            }),
        )
    }

    fn parse_param_name(&mut self) -> TermName {
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                match self.intern_current_term_name() {
                    Ok(name) => {
                        self.advance();
                        name
                    }
                    Err(_) => self.missing_param_name(),
                }
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a parameter name",
                );
                self.missing_param_name()
            }
        }
    }

    fn missing_param_name(&mut self) -> TermName {
        let index = self.next_wildcard_param;
        self.next_wildcard_param = self.next_wildcard_param.saturating_add(1);
        TermName::new(self.names.intern(&format!("$missing_param_{index}")))
    }

    fn recover_term_param_clause(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::Eof | TokenKind::Punctuation(Punctuation::RightParen)
        ) && !(self.context.enum_body
            && matches!(
                self.current().kind,
                TokenKind::Newline
                    | TokenKind::Newlines
                    | TokenKind::Indent
                    | TokenKind::Outdent
                    | TokenKind::Keyword(dotty_core::HardKeyword::Case)
                    | TokenKind::Punctuation(Punctuation::RightBrace)
            ))
            && !(self.context.block_end.is_some()
                && matches!(
                    self.current().kind,
                    TokenKind::Newline
                        | TokenKind::Newlines
                        | TokenKind::Outdent
                        | TokenKind::Punctuation(Punctuation::RightBrace | Punctuation::Semicolon)
                ))
        {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
        self.accept(TokenKind::Punctuation(Punctuation::RightParen));
    }
}

fn is_parameter_colon<S: dotty_core::TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    is_parameter_colon_at(parser, 0)
}

fn is_parameter_colon_at<S: dotty_core::TokenSource>(
    parser: &mut Parser<'_, '_, S>,
    offset: usize,
) -> bool {
    let token = parser.cursor.lookahead(offset).clone();
    matches!(
        token.kind,
        TokenKind::ColonFollow
            | TokenKind::ColonEol
            | TokenKind::ColonOp
            | TokenKind::Punctuation(Punctuation::Colon)
    ) && parser.token_text(&token).ok() == Some(":")
}

fn current_is_anonymous_context_type<S: dotty_core::TokenSource>(
    parser: &mut Parser<'_, '_, S>,
) -> bool {
    parser.current().kind != TokenKind::Punctuation(Punctuation::RightParen)
        && !is_parameter_colon_at(parser, 1)
        && !parser.current_is_inline_parameter_modifier()
}

fn is_class_parameter_owner(owner: ParamOwner) -> bool {
    matches!(owner, ParamOwner::Class | ParamOwner::CaseClass)
}

fn add_class_parameter_metadata(
    owner: ParamOwner,
    first_ordinary_clause: bool,
    metadata: &mut Modifiers,
) {
    if !is_class_parameter_owner(owner) {
        return;
    }

    metadata.modifiers.push(Modifier::ParamAccessor);
    if owner != ParamOwner::CaseClass || !first_ordinary_clause {
        metadata.modifiers.push(Modifier::PrivateLocal);
    }
}

fn is_bare_assignment<S: dotty_core::TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    parser.current().kind == TokenKind::Operator && parser.current_text_is("=")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::NameInterner;
    use dotty_core::ast::UntypedNode;

    #[test]
    fn parses_an_empty_term_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "()",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::RightParen), 1, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses, vec![Vec::new()]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_typed_parameters_as_val_defs_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A, y: pkg.B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonFollow, 8, 9),
                token(TokenKind::Identifier, 10, 13),
                token(TokenKind::Punctuation(Punctuation::Dot), 13, 14),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 2);
        let first = parser.ast().get(clauses[0][0]).kind.clone();
        let second = parser.ast().get(clauses[0][1]).kind.clone();
        let first_name = match first {
            TreeKind::ValDef(ValDef {
                name, rhs: None, ..
            }) => name,
            _ => panic!("expected first parameter ValDef"),
        };
        let second_name = match second {
            TreeKind::ValDef(ValDef {
                name, rhs: None, ..
            }) => name,
            _ => panic!("expected second parameter ValDef"),
        };
        assert_eq!(parser.names.resolve(first_name.as_name().text()), "x");
        assert_eq!(parser.names.resolve(second_name.as_name().text()), "y");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_repeated_parameter_type_as_a_postfix_star_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(xs: A*)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 3),
                token(TokenKind::ColonFollow, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected parameter ValDef");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PostfixOp(repeated)) =
            &parser.ast().get(parameter.tpt).kind
        else {
            panic!("expected repeated parameter type to be a PostfixOp");
        };

        assert_eq!(parser.names.resolve(repeated.op.text()), "*");
        assert_eq!(
            parser
                .ast()
                .get(repeated.operand)
                .position
                .unwrap()
                .span()
                .range()
                .start(),
            5
        );
        assert_eq!(
            parser
                .ast()
                .get(parameter.tpt)
                .position
                .unwrap()
                .span()
                .range()
                .end(),
            7
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_repeated_applied_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(xs: List[A]*)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 3),
                token(TokenKind::ColonFollow, 3, 4),
                token(TokenKind::Identifier, 5, 9),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Operator, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected parameter ValDef");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PostfixOp(repeated)) =
            &parser.ast().get(parameter.tpt).kind
        else {
            panic!("expected repeated parameter type to be a PostfixOp");
        };

        assert!(matches!(
            parser.ast().get(repeated.operand).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_and_diagnoses_repeated_types_in_named_given_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(xs: A*)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 3),
                token(TokenKind::ColonFollow, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let clause = parser.parse_single_term_param_clause(ParamOwner::Given, true, false, 0);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clause[0]).kind else {
            panic!("expected named given parameter ValDef");
        };

        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_))
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
    fn diagnoses_repeated_types_in_legacy_implicit_parameter_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(implicit xs: A*)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Implicit), 1, 9),
                token(TokenKind::Identifier, 10, 12),
                token(TokenKind::ColonFollow, 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected implicit parameter ValDef");
        };

        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_))
        ));
        assert!(parameter.metadata.modifiers.contains(&Modifier::Implicit));
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken
                && diagnostic
                    .message()
                    .contains("not allowed in `given` or `implicit`")
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_a_followed_star_as_an_infix_type_operator_in_a_parameter_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A * B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected parameter ValDef");
        };

        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_repeated_parameter_that_is_not_last_in_its_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(xs: A*, y: B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 3),
                token(TokenKind::ColonFollow, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 2);
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind() == ParseDiagnosticKind::UnexpectedToken
                && diagnostic.message().contains("must come last")
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_inline_on_method_parameters_without_affecting_neighbors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(inline op: Boolean, value: Int)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 19),
                token(TokenKind::Punctuation(Punctuation::Comma), 19, 20),
                token(TokenKind::Identifier, 21, 26),
                token(TokenKind::ColonFollow, 26, 27),
                token(TokenKind::Identifier, 28, 31),
                token(TokenKind::Punctuation(Punctuation::RightParen), 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let first = match &parser.ast().get(clauses[0][0]).kind {
            TreeKind::ValDef(parameter) => parameter,
            _ => panic!("expected first parameter ValDef"),
        };
        let second = match &parser.ast().get(clauses[0][1]).kind {
            TreeKind::ValDef(parameter) => parameter,
            _ => panic!("expected second parameter ValDef"),
        };

        assert!(first.metadata.modifiers.contains(&Modifier::Inline));
        assert!(first.metadata.modifiers.contains(&Modifier::Param));
        assert_eq!(second.metadata.modifiers, vec![Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_inline_on_constructor_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(inline x: Boolean)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::ColonFollow, 9, 10),
                token(TokenKind::Identifier, 11, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected constructor parameter ValDef");
        };

        assert!(parameter.metadata.modifiers.contains(&Modifier::Inline));
        assert!(
            parameter
                .metadata
                .modifiers
                .contains(&Modifier::ParamAccessor)
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_inline_on_named_using_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using inline context: Context)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Identifier, 14, 21),
                token(TokenKind::ColonFollow, 21, 22),
                token(TokenKind::Identifier, 23, 30),
                token(TokenKind::Punctuation(Punctuation::RightParen), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected using parameter ValDef");
        };

        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(parameter.metadata.modifiers.contains(&Modifier::Inline));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_inline_on_named_given_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using inline evidence: Evidence)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 13),
                token(TokenKind::Identifier, 14, 22),
                token(TokenKind::ColonFollow, 22, 23),
                token(TokenKind::Identifier, 24, 32),
                token(TokenKind::Punctuation(Punctuation::RightParen), 32, 33),
                token(TokenKind::Eof, 33, 33),
            ],
            &mut names,
        );

        let clause = parser.parse_single_term_param_clause(ParamOwner::Given, true, true, 0);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clause[0]).kind else {
            panic!("expected given parameter ValDef");
        };

        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(parameter.metadata.modifiers.contains(&Modifier::Inline));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_inline_as_a_parameter_name_when_followed_by_a_colon() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(inline: Boolean)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected parameter ValDef");
        };

        assert_eq!(
            parser.names.resolve(parameter.name.as_name().text()),
            "inline"
        );
        assert!(!parameter.metadata.modifiers.contains(&Modifier::Inline));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnoses_an_inline_parameter_without_a_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(inline value)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::Identifier, 8, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn accepts_a_newline_terminated_trailing_comma_in_a_term_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A,\n)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_trailing_comma_without_a_line_break_in_a_term_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A,)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn still_reports_a_missing_parameter_before_a_non_trailing_comma() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A,, y: B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Punctuation(Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::ColonFollow, 9, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::Punctuation(Punctuation::RightParen), 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let _clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_trailing_comma_in_a_named_using_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using context: A,)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 14),
                token(TokenKind::ColonFollow, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::Comma), 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn accepts_a_newline_terminated_trailing_comma_in_a_named_using_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using context: A,\n)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 14),
                token(TokenKind::ColonFollow, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::Comma), 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn marks_a_plain_class_parameter_as_accessor_and_private_local() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected a constructor parameter");
        };
        assert_eq!(
            metadata.modifiers,
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn marks_an_explicit_class_val_parameter_as_an_accessor() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(val x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 1, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let parameter = clauses[0][0];
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(parameter).kind else {
            panic!("expected a constructor parameter");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::ParamAccessor]);
        assert_eq!(
            parser
                .ast()
                .get(parameter)
                .position
                .unwrap()
                .span()
                .range()
                .start(),
            1
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn marks_an_explicit_class_var_parameter_as_a_mutable_accessor() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(var x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Var), 1, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected a constructor parameter");
        };
        assert_eq!(
            metadata.modifiers,
            vec![Modifier::ParamAccessor, Modifier::Var]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_final_on_class_val_and_var_accessors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(final val x: A, final var y: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Final), 1, 6),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 7, 10),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::ColonFollow, 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::Comma), 15, 16),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Final), 17, 22),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Var), 23, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::ColonFollow, 28, 29),
                token(TokenKind::Identifier, 30, 31),
                token(TokenKind::Punctuation(Punctuation::RightParen), 31, 32),
                token(TokenKind::Eof, 32, 32),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(immutable) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a class val parameter");
        };
        let TreeKind::ValDef(mutable) = &parser.ast().get(clauses[0][1]).kind else {
            panic!("expected a class var parameter");
        };

        assert_eq!(
            immutable.metadata.modifiers,
            vec![Modifier::Final, Modifier::ParamAccessor]
        );
        assert_eq!(
            mutable.metadata.modifiers,
            vec![Modifier::Final, Modifier::ParamAccessor, Modifier::Var]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_final_and_protected_in_either_order_on_class_accessors() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(final protected val x: A, protected final val y: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Final), 1, 6),
                token(
                    TokenKind::Keyword(dotty_core::HardKeyword::Protected),
                    7,
                    16,
                ),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 17, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::ColonFollow, 22, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Punctuation(Punctuation::Comma), 25, 26),
                token(
                    TokenKind::Keyword(dotty_core::HardKeyword::Protected),
                    27,
                    36,
                ),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Final), 37, 42),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 43, 46),
                token(TokenKind::Identifier, 47, 48),
                token(TokenKind::ColonFollow, 48, 49),
                token(TokenKind::Identifier, 50, 51),
                token(TokenKind::Punctuation(Punctuation::RightParen), 51, 52),
                token(TokenKind::Eof, 52, 52),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        for parameter_id in &clauses[0] {
            let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter_id).kind else {
                panic!("expected a class accessor parameter");
            };
            assert_eq!(
                parameter.metadata.modifiers,
                vec![Modifier::Final, Modifier::ParamAccessor]
            );
            assert_eq!(
                parameter.metadata.visibility,
                Some(dotty_core::ast::VisibilitySyntax::Protected { qualifier: None })
            );
        }
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn requires_an_accessor_after_final_on_a_class_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(final x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Final), 1, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonFollow, 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);

        assert_eq!(clauses[0].len(), 1);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(parser.diagnostics()[0].message(), "`val` or `var` expected");
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_annotations_on_method_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(@Marker x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Operator, 1, 2),
                token(TokenKind::Identifier, 2, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a method parameter");
        };

        assert_eq!(parameter.metadata.annotations.len(), 1);
        assert_eq!(parser.diagnostics().len(), 0);
        assert_eq!(
            parser
                .ast()
                .get(clauses[0][0])
                .position
                .unwrap()
                .span()
                .range(),
            dotty_core::TextRange::new(1, 13).unwrap()
        );
    }

    #[test]
    fn parses_a_by_name_type_on_a_named_method_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(action: => Unit)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a method parameter");
        };

        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(parameter.tpt).kind else {
            panic!("expected a by-name parameter type");
        };
        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(
            parser
                .ast()
                .get(parameter.tpt)
                .position
                .unwrap()
                .span()
                .range(),
            dotty_core::TextRange::new(9, 16).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_qualified_applied_types_after_a_by_name_arrow() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(value: => pkg.Type[Arg])",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Identifier, 11, 14),
                token(TokenKind::Punctuation(Punctuation::Dot), 14, 15),
                token(TokenKind::Identifier, 15, 19),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 19, 20),
                token(TokenKind::Identifier, 20, 23),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 23, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a method parameter");
        };
        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(parameter.tpt).kind else {
            panic!("expected a by-name parameter type");
        };

        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert_eq!(
            parser
                .ast()
                .get(parameter.tpt)
                .position
                .unwrap()
                .span()
                .range(),
            dotty_core::TextRange::new(8, 24).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_by_name_type_on_a_class_constructor_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(action: => Unit)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a constructor parameter");
        };

        assert!(matches!(
            parser.ast().get(parameter.tpt).kind,
            TreeKind::ByNameTypeTree(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn diagnoses_a_by_name_type_on_a_val_constructor_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(val action: => Unit)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 1, 4),
                token(TokenKind::Identifier, 5, 11),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 20),
                token(TokenKind::Punctuation(Punctuation::RightParen), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);

        assert_eq!(clauses[0].len(), 1);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        assert_eq!(
            parser.diagnostics()[0].message(),
            "`val` parameters may not be call-by-name"
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            dotty_core::TextRange::new(13, 15).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn diagnoses_a_by_name_type_on_a_var_constructor_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(var action: => Unit)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Var), 1, 4),
                token(TokenKind::Identifier, 5, 11),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Operator, 13, 15),
                token(TokenKind::Identifier, 16, 20),
                token(TokenKind::Punctuation(Punctuation::RightParen), 20, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);

        assert_eq!(clauses[0].len(), 1);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
        assert_eq!(
            parser.diagnostics()[0].message(),
            "`var` parameters may not be call-by-name"
        );
        assert_eq!(
            parser.diagnostics()[0].span(),
            dotty_core::TextRange::new(13, 15).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn diagnoses_a_by_name_type_on_an_implicit_case_class_accessor() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(action: => Unit)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::CaseClass);

        assert_eq!(clauses[0].len(), 1);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].message(),
            "`val` parameters may not be call-by-name"
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn a_missing_by_name_result_preserves_the_next_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(first: =>, second: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Operator, 8, 10),
                token(TokenKind::Punctuation(Punctuation::Comma), 10, 11),
                token(TokenKind::Identifier, 12, 18),
                token(TokenKind::ColonFollow, 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Punctuation(Punctuation::RightParen), 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 2);
        let TreeKind::ValDef(first) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected the first parameter");
        };
        let TreeKind::ByNameTypeTree(by_name) = &parser.ast().get(first.tpt).kind else {
            panic!("expected a partial by-name parameter type");
        };
        assert!(matches!(
            parser.ast().get(by_name.result).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        let TreeKind::ValDef(second) = &parser.ast().get(clauses[0][1]).kind else {
            panic!("expected the next parameter to survive recovery");
        };
        assert_eq!(parser.names.resolve(second.name.as_name().text()), "second");
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::ExpectedType })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn a_missing_by_name_result_does_not_consume_the_parameter_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(action: =>)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::ExpectedType })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn preserves_visibility_on_class_accessor_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(private val x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Private), 1, 8),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 9, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::ColonFollow, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightParen), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a constructor parameter");
        };

        assert_eq!(
            parameter.metadata.visibility,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: None })
        );
        assert!(
            parameter
                .metadata
                .modifiers
                .contains(&Modifier::ParamAccessor)
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_override_on_class_accessor_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(override val x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Override), 1, 9),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 10, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::ColonFollow, 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected a constructor parameter");
        };

        assert!(parameter.metadata.modifiers.contains(&Modifier::Override));
        assert!(
            parameter
                .metadata
                .modifiers
                .contains(&Modifier::ParamAccessor)
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn requires_an_explicit_accessor_after_override_on_class_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(override x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Override), 1, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
        assert_eq!(parser.diagnostics()[0].message(), "`val` or `var` expected");
    }

    #[test]
    fn reports_duplicate_class_parameter_visibility_and_recovers() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(private private val x: A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Private), 1, 8),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Private), 9, 16),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Val), 17, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::ColonFollow, 22, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Punctuation(Punctuation::RightParen), 25, 26),
                token(TokenKind::Eof, 26, 26),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Class);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 1);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
    }

    #[test]
    fn later_case_class_clauses_keep_accessor_and_private_local_roles() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A)(y: B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonFollow, 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::CaseClass);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected first parameter");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::ParamAccessor]);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[1][0]).kind
        else {
            panic!("expected second parameter");
        };
        assert_eq!(
            metadata.modifiers,
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_boundaries_between_multiple_term_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A)(y: B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonFollow, 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].len(), 1);
        assert_eq!(clauses[1].len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_missing_parameter_type_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 1);
        assert!(matches!(
            parser.ast().get(match clauses[0].as_slice() {
                [parameter] => *parameter,
                _ => panic!("expected one parameter"),
            }).kind,
            TreeKind::ValDef(ValDef { tpt, .. })
                if matches!(parser.ast().get(tpt).kind, TreeKind::PhaseSpecific(UntypedNode::Error(_)))
        ));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedType)
        );
    }

    #[test]
    fn parses_a_named_using_clause_with_given_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using ctx: Ctx)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 1);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected a parameter ValDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Given, Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_regular_then_using_clause_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A)(using ctx: Ctx)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Identifier, 13, 16),
                token(TokenKind::ColonFollow, 16, 17),
                token(TokenKind::Identifier, 18, 21),
                token(TokenKind::Punctuation(Punctuation::RightParen), 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 2);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected ordinary parameter ValDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Param]);
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[1][0]).kind
        else {
            panic!("expected using parameter ValDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Given, Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_empty_using_clause_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses, vec![Vec::new()]);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
    }

    #[test]
    fn parses_a_legacy_implicit_parameter_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(implicit ctx: Ctx)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Implicit), 1, 8),
                token(TokenKind::Identifier, 10, 13),
                token(TokenKind::ColonFollow, 13, 14),
                token(TokenKind::Identifier, 15, 18),
                token(TokenKind::Punctuation(Punctuation::RightParen), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 1);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[0][0]).kind else {
            panic!("expected an implicit parameter ValDef");
        };
        let parameter_name = *parameter.name.as_name();
        assert!(parameter.metadata.modifiers.contains(&Modifier::Implicit));
        assert!(!parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(
            parser.diagnostics().is_empty(),
            "unexpected diagnostics: {:?}",
            parser.diagnostics()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        drop(parser);
        assert_eq!(names.resolve(parameter_name.text()), "ctx");
    }

    #[test]
    fn legacy_implicit_clause_follows_regular_clause_and_ends_clause_sequence() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A)(implicit ctx: Ctx)(y: B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 6, 7),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Implicit), 7, 15),
                token(TokenKind::Identifier, 16, 19),
                token(TokenKind::ColonFollow, 19, 20),
                token(TokenKind::Identifier, 21, 24),
                token(TokenKind::Punctuation(Punctuation::RightParen), 24, 25),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 25, 26),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::ColonFollow, 27, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Punctuation(Punctuation::RightParen), 30, 31),
                token(TokenKind::Eof, 31, 31),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 2);
        assert_eq!(clauses[0].len(), 1);
        assert_eq!(clauses[1].len(), 1);
        let TreeKind::ValDef(parameter) = &parser.ast().get(clauses[1][0]).kind else {
            panic!("expected the implicit parameter ValDef");
        };
        assert!(parameter.metadata.modifiers.contains(&Modifier::Implicit));
        assert_eq!(
            parser.current().kind,
            TokenKind::Punctuation(Punctuation::LeftParen)
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_empty_legacy_implicit_clause_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(implicit)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Implicit), 1, 9),
                token(TokenKind::Punctuation(Punctuation::RightParen), 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses, vec![Vec::new()]);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
    }

    #[test]
    fn parses_an_anonymous_using_clause_with_a_synthetic_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using Ctx)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].len(), 1);
        let TreeKind::ValDef(ValDef {
            name, ref metadata, ..
        }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected an anonymous using parameter");
        };
        assert_eq!(parser.names.resolve(name.as_name().text()), "x$1");
        assert_eq!(metadata.modifiers, vec![Modifier::Given, Modifier::Param]);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_multiple_anonymous_using_types_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using A, B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightParen), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 2);
        let names = clauses[0]
            .iter()
            .map(|parameter| match &parser.ast().get(*parameter).kind {
                TreeKind::ValDef(value) => parser.names.resolve(value.name.as_name().text()),
                _ => panic!("expected an anonymous using parameter"),
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["x$1", "x$2"]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_a_newline_terminated_trailing_comma_in_anonymous_using_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using Context,\n)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 14),
                token(TokenKind::Punctuation(Punctuation::Comma), 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_same_line_trailing_comma_in_anonymous_using_clause() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using Context, )",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 14),
                token(TokenKind::Punctuation(Punctuation::Comma), 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_a_missing_anonymous_using_clause_delimiter_without_hanging() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(using Context",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::Identifier, 7, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedToken)
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn anonymous_using_names_follow_leading_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(value: Value)(using Context)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Identifier, 8, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 14, 15),
                token(TokenKind::Identifier, 15, 20),
                token(TokenKind::Identifier, 21, 28),
                token(TokenKind::Punctuation(Punctuation::RightParen), 28, 29),
                token(TokenKind::Eof, 29, 29),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        let TreeKind::ValDef(value) = &parser.ast().get(clauses[1][0]).kind else {
            panic!("expected an anonymous using parameter");
        };
        assert_eq!(parser.names.resolve(value.name.as_name().text()), "x$2");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_a_default_parameter_value() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A = default)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        let TreeKind::ValDef(ValDef { rhs: Some(rhs), .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected a parameter default");
        };
        assert!(matches!(parser.ast().get(rhs).kind, TreeKind::Ident(_)));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn stops_a_default_expression_at_the_next_parameter_comma() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(x: A = one + two, y: B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::ColonFollow, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Operator, 12, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Punctuation(Punctuation::Comma), 17, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::ColonFollow, 20, 21),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Punctuation(Punctuation::RightParen), 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses[0].len(), 2);
        let TreeKind::ValDef(ValDef { rhs: Some(rhs), .. }) = parser.ast().get(clauses[0][0]).kind
        else {
            panic!("expected a parameter default");
        };
        assert!(matches!(
            parser.ast().get(rhs).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        let TreeKind::ValDef(ValDef { rhs: None, .. }) = parser.ast().get(clauses[0][1]).kind
        else {
            panic!("expected a parameter without a default");
        };
        assert!(parser.diagnostics().is_empty());
    }
}
