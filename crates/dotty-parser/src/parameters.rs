//! Reusable source-level term parameter clauses.
//!
//! Parameters are represented by the existing [`dotty_core::ast::ValDef`]
//! node. This module owns the clause/parameter boundary so definitions,
//! constructors, givens, and extensions can reuse it without duplicating the
//! delimiter and recovery logic.

use dotty_core::ast::{Modifier, Modifiers, ValDef};
use dotty_core::{Punctuation, TermName, TokenKind, TreeId, TreeKind, Untyped};

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
            loop {
                parser.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
                    Punctuation::LeftParen,
                ));
                if parser.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
                    break;
                }
                if parser.current_is_unsupported_parameter_clause() {
                    clauses.push(parser.parse_unsupported_term_param_clause());
                    continue;
                }
                let is_using = parser.current_is_using_parameter_clause();
                clauses.push(if is_using {
                    parser.parse_term_param_clause_with_policy(owner, false, true)
                } else {
                    let clause = parser.parse_term_param_clause_with_policy(
                        owner,
                        first_ordinary_clause,
                        false,
                    );
                    first_ordinary_clause = false;
                    clause
                });
            }
            clauses
        })
    }

    fn parse_unsupported_term_param_clause(&mut self) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftParen));
        let message =
            if self.current().kind == TokenKind::Keyword(dotty_core::HardKeyword::Implicit) {
                "legacy `implicit` parameter clauses are not supported; use a named `using` clause"
            } else {
                "anonymous `using` parameter clauses are not supported; name the context parameter"
            };
        self.report(ParseDiagnosticKind::UnsupportedSyntax, message);
        self.recover_term_param_clause();
        Vec::new()
    }

    /// Parses one term parameter clause for a grammar production that needs
    /// to enforce its own clause ordering, such as `extension`.
    pub(crate) fn parse_single_term_param_clause(
        &mut self,
        owner: ParamOwner,
        first_ordinary_clause: bool,
        is_using: bool,
    ) -> Vec<TreeId<Untyped>> {
        if self.current_is_unsupported_parameter_clause() {
            self.parse_unsupported_term_param_clause()
        } else {
            self.parse_term_param_clause_with_policy(owner, first_ordinary_clause, is_using)
        }
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
    ) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftParen));
        let mut params = Vec::new();
        let mut metadata = Modifiers::default();

        if is_using || owner == ParamOwner::Given {
            metadata.modifiers.push(Modifier::Given);
        }

        if is_using {
            self.advance();
            if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a parameter after `using`",
                );
                return params;
            }
        }

        if !is_using && self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
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

            if self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected a parameter after `,`",
                    );
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
            if owner == ParamOwner::CaseClass && first_ordinary_clause {
                metadata.modifiers.push(Modifier::ParamAccessor);
            } else {
                metadata.modifiers.push(Modifier::ParamAccessor);
                metadata.modifiers.push(Modifier::PrivateLocal);
            }
        }
        let name = self.parse_param_name();
        let tpt = if is_parameter_colon(self) {
            self.advance();
            self.with_parse_kind(ParseKind::Type, |parser| parser.type_expr())
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected `:` and a parameter type",
            );
            self.error_type(self.current_span())
        };
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

    pub(crate) fn current_is_unsupported_parameter_clause(&mut self) -> bool {
        if self.current().kind != TokenKind::Punctuation(Punctuation::LeftParen) {
            return false;
        }
        if self.cursor.lookahead(1).kind == TokenKind::Keyword(dotty_core::HardKeyword::Implicit) {
            return true;
        }
        if !self.current_is_using_parameter_clause() {
            return false;
        }
        matches!(
            self.cursor.lookahead(2).kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) && !is_parameter_colon_at(self, 3)
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

fn is_class_parameter_owner(owner: ParamOwner) -> bool {
    matches!(owner, ParamOwner::Class | ParamOwner::CaseClass)
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
        assert_eq!(metadata.modifiers, vec![Modifier::Given]);
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
        assert!(metadata.modifiers.is_empty());
        let TreeKind::ValDef(ValDef { ref metadata, .. }) = parser.ast().get(clauses[1][0]).kind
        else {
            panic!("expected using parameter ValDef");
        };
        assert_eq!(metadata.modifiers, vec![Modifier::Given]);
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
    fn reports_legacy_implicit_clauses_as_explicitly_unsupported() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(implicit ctx: Ctx)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Keyword(dotty_core::HardKeyword::Implicit), 1, 8),
                token(TokenKind::Identifier, 9, 12),
                token(TokenKind::ColonFollow, 12, 13),
                token(TokenKind::Identifier, 14, 17),
                token(TokenKind::Punctuation(Punctuation::RightParen), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let clauses = parser.parse_term_param_clauses(ParamOwner::Def);

        assert_eq!(clauses, vec![Vec::new()]);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::UnsupportedSyntax })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn reports_anonymous_using_clauses_as_explicitly_unsupported() {
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

        assert_eq!(clauses, vec![Vec::new()]);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::UnsupportedSyntax })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
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
