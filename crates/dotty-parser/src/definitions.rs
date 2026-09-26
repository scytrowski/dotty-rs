//! Parser support for the first, simple value-definition forms.
//!
//! Keeping the statement-level dispatch here means that `val` and `var` do not
//! get mistaken for unsupported top-level expressions while the shared
//! statement-sequence machinery remains grammar-agnostic.

use dotty_core::ast::{DefDef, Modifier, Modifiers, PatDef, TypeTree, UntypedNode, ValDef};
use dotty_core::{
    HardKeyword, SourceSpan, Span, TermName, TextRange, TokenKind, TreeId, TreeKind, Untyped,
};

use crate::modifiers::DefinitionPrefix;
use crate::statements::ParsedStatement;
use crate::{Location, ParseDiagnosticKind, Parser, RecoverySet};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_value_definition(&mut self, location: Location) -> ParsedStatement {
        let prefix = DefinitionPrefix::empty(self.mark().start());
        self.parse_value_definition_with_prefix(location, prefix)
    }

    pub(crate) fn parse_value_definition_with_prefix(
        &mut self,
        location: Location,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        let is_var = self.current().kind == TokenKind::Keyword(HardKeyword::Var);
        self.advance();

        if !starts_simple_value_definition(self) {
            return self.parse_pattern_definition(mark, is_var, location, prefix.metadata);
        }

        self.parse_simple_value_definition(mark, is_var, location, prefix.metadata)
    }

    pub(crate) fn parse_method_definition(&mut self, location: Location) -> ParsedStatement {
        let prefix = DefinitionPrefix::empty(self.mark().start());
        self.parse_method_definition_with_prefix(location, prefix)
    }

    pub(crate) fn parse_method_definition_with_prefix(
        &mut self,
        location: Location,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        let mark = crate::Mark {
            start: prefix.start,
        };
        self.advance();

        let name = self.parse_method_name();
        self.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
            dotty_core::Punctuation::LeftBracket,
        ));
        let type_params = if self.current().kind
            == TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket)
        {
            self.parse_type_param_clause(crate::ParamOwner::Def)
        } else {
            Vec::new()
        };

        let value_param_clauses = self.parse_term_param_clauses(crate::ParamOwner::Def);
        self.consume_newlines_before_parameter_clause(TokenKind::Punctuation(
            dotty_core::Punctuation::LeftBracket,
        ));
        let has_interleaved_type_params =
            self.current().kind == TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket);
        if has_interleaved_type_params {
            self.report(
                ParseDiagnosticKind::UnsupportedSyntax,
                "interleaved type parameter clauses are not supported; put type parameters before term clauses",
            );
            self.recover_until(RecoverySet::Statement);
        }
        let return_type_start = self.last_real_token_end;
        let has_explicit_return_type = !has_interleaved_type_params && is_definition_colon(self);
        let tpt = if has_explicit_return_type {
            self.advance();
            self.with_parse_kind(crate::ParseKind::Type, |parser| parser.type_expr())
        } else {
            synthetic_type_tree(self, return_type_start)
        };

        let rhs = if has_interleaved_type_params {
            None
        } else if is_bare_assignment(self) {
            let feedback_opened = self.observe_indented_body();
            self.advance();
            Some(self.parse_method_rhs(location, feedback_opened))
        } else if has_explicit_return_type && is_definition_boundary(self.current().kind) {
            None
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `:` or `=` after a method definition",
            );
            None
        };

        let definition = self.alloc_from(
            mark,
            TreeKind::DefDef(DefDef {
                name,
                type_params,
                value_param_clauses,
                tpt,
                rhs,
                metadata: prefix.metadata,
            }),
        );
        ParsedStatement::Definition(definition)
    }

    fn parse_method_rhs(&mut self, location: Location, feedback_opened: bool) -> TreeId<Untyped> {
        self.consume_control_newlines();
        self.with_secondary_constructor_allowed(false, |parser| {
            if parser.current().kind == TokenKind::Indent {
                if feedback_opened {
                    parser.parse_feedback_indented_block()
                } else {
                    parser.parse_indented_block()
                }
            } else {
                parser.with_location(location, |parser| parser.expr())
            }
        })
    }

    fn parse_method_name(&mut self) -> TermName {
        let secondary_constructor_allowed = self.context.secondary_constructor_allowed;
        self.context.secondary_constructor_allowed = false;
        match self.current().kind {
            TokenKind::Keyword(HardKeyword::This) => {
                self.advance();
                if secondary_constructor_allowed {
                    TermName::new(self.names.intern("<init>"))
                } else {
                    self.report(
                        ParseDiagnosticKind::UnexpectedToken,
                        "secondary constructors are only allowed in a class template",
                    );
                    self.missing_method_name()
                }
            }
            TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::Operator
            | TokenKind::ColonOp => match self.intern_current_term_name() {
                Ok(name) => {
                    self.advance();
                    name
                }
                Err(_) => self.missing_method_name(),
            },
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a method name after `def`",
                );
                self.missing_method_name()
            }
        }
    }

    fn missing_method_name(&mut self) -> TermName {
        TermName::new(self.names.intern("$missing_method"))
    }

    fn parse_simple_value_definition(
        &mut self,
        mark: crate::Mark,
        is_var: bool,
        location: Location,
        mut metadata: Modifiers,
    ) -> ParsedStatement {
        let name = match self.current().kind {
            TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::Operator
            | TokenKind::ColonOp => {
                if self.current().kind == TokenKind::Operator && self.current_text_is("=") {
                    self.report(
                        ParseDiagnosticKind::ExpectedToken,
                        "expected a value name after `val` or `var`",
                    );
                    return self.malformed_definition();
                }
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

        let type_start = self.last_real_token_end;
        let has_explicit_type = is_definition_colon(self);
        let tpt = if has_explicit_type {
            self.advance();
            self.with_location(location, |parser| {
                parser.with_parse_kind(crate::ParseKind::Type, |parser| parser.type_expr())
            })
        } else {
            synthetic_type_tree(self, type_start)
        };

        let rhs = if is_bare_assignment(self) {
            self.advance();
            Some(self.with_secondary_constructor_allowed(false, |parser| {
                parser.with_location(location, |parser| parser.expr())
            }))
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

        if is_var {
            metadata.modifiers.push(Modifier::Var);
        }
        let definition = self.alloc_from(
            mark,
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs,
                metadata,
            }),
        );
        ParsedStatement::Definition(definition)
    }

    fn parse_pattern_definition(
        &mut self,
        mark: crate::Mark,
        is_var: bool,
        location: Location,
        mut metadata: Modifiers,
    ) -> ParsedStatement {
        if is_definition_boundary(self.current().kind) {
            self.report(
                ParseDiagnosticKind::ExpectedPattern,
                "expected a pattern after `val` or `var`",
            );
            return self.malformed_definition();
        }

        let mut patterns = Vec::new();
        let first = self.with_parse_kind(crate::ParseKind::Pattern, |parser| {
            parser.with_location(Location::InPattern, |parser| parser.pattern2())
        });
        let first_is_identifier = matches!(&self.ast().get(first).kind, TreeKind::Ident(_));
        patterns.push(first);

        if self.accept(TokenKind::Punctuation(dotty_core::Punctuation::Comma)) {
            if !first_is_identifier {
                self.report(
                    ParseDiagnosticKind::ExpectedPattern,
                    "only simple identifiers may be comma-separated in a value definition",
                );
            } else {
                loop {
                    if !is_comma_definition_identifier(self) {
                        self.report(
                            ParseDiagnosticKind::ExpectedPattern,
                            "expected an identifier after `,` in a value definition",
                        );
                        break;
                    }
                    patterns.push(self.with_parse_kind(crate::ParseKind::Pattern, |parser| {
                        parser.with_location(Location::InPattern, |parser| parser.pattern2())
                    }));
                    if !self.accept(TokenKind::Punctuation(dotty_core::Punctuation::Comma)) {
                        break;
                    }
                }
            }
        }

        let all_simple_identifiers = patterns
            .iter()
            .all(|pattern| matches!(&self.ast().get(*pattern).kind, TreeKind::Ident(_)));
        let has_explicit_type = is_definition_colon(self);
        let type_start = self.last_real_token_end;
        let tpt = if has_explicit_type {
            self.advance();
            self.with_parse_kind(crate::ParseKind::Type, |parser| parser.type_expr())
        } else {
            synthetic_type_tree(self, type_start)
        };

        let rhs = if is_bare_assignment(self) {
            self.advance();
            Some(self.with_secondary_constructor_allowed(false, |parser| {
                parser.with_location(location, |parser| parser.expr())
            }))
        } else if has_explicit_type
            && all_simple_identifiers
            && is_definition_boundary(self.current().kind)
        {
            None
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected `=` after a pattern definition",
            );
            Some(self.error_expr(self.current_span()))
        };

        if is_var {
            metadata.modifiers.push(Modifier::Var);
        }
        let definition = self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::PatDef(PatDef {
                modifiers: metadata,
                patterns,
                tpt,
                rhs,
            })),
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

fn starts_simple_value_definition<S: dotty_core::TokenSource>(
    parser: &mut Parser<'_, '_, S>,
) -> bool {
    let kind = parser.current().kind;
    let symbolic_name = matches!(kind, TokenKind::Operator | TokenKind::ColonOp);
    if !matches!(
        kind,
        TokenKind::Identifier | TokenKind::BackquotedIdentifier
    ) && !symbolic_name
    {
        return false;
    }
    if symbolic_name && parser.current_text_is("=") {
        return false;
    }

    let next = parser.cursor.lookahead(1).kind;
    is_definition_boundary(next)
        || is_definition_colon_at(parser, 1)
        || (next == TokenKind::Operator && {
            let token = parser.cursor.lookahead(1).clone();
            parser.token_text(&token).ok() == Some("=")
        })
}

fn is_definition_colon_at<S: dotty_core::TokenSource>(
    parser: &mut Parser<'_, '_, S>,
    offset: usize,
) -> bool {
    let token = parser.cursor.lookahead(offset).clone();
    matches!(
        token.kind,
        TokenKind::ColonFollow
            | TokenKind::ColonEol
            | TokenKind::ColonOp
            | TokenKind::Punctuation(dotty_core::Punctuation::Colon)
    ) && parser.token_text(&token).ok() == Some(":")
}

fn is_comma_definition_identifier<S: dotty_core::TokenSource>(
    parser: &mut Parser<'_, '_, S>,
) -> bool {
    if !matches!(
        parser.current().kind,
        TokenKind::Identifier | TokenKind::BackquotedIdentifier
    ) {
        return false;
    }

    let next = parser.cursor.lookahead(1).kind;
    is_definition_boundary(next)
        || is_definition_colon_at(parser, 1)
        || (next == TokenKind::Punctuation(dotty_core::Punctuation::Comma))
        || (next == TokenKind::Operator && {
            let token = parser.cursor.lookahead(1).clone();
            parser.token_text(&token).ok() == Some("=")
        })
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
    use dotty_core::{NameInterner, Punctuation, TokenKind, TreeKind};

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
    fn parses_a_symbolic_operator_as_a_typed_value_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val * : N = \"*\"",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::StringLiteral, 12, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a value definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected Dotty's source-level ValDef shape");
        };
        let name_id = definition.name.as_name().text();
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(matches!(
            parser.ast().get(*definition.rhs.as_ref().unwrap()).kind,
            TreeKind::Literal(_)
        ));
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "*");
    }

    #[test]
    fn parses_a_symbolic_operator_as_an_inferred_value_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val + = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::IntegerLiteral, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a value definition");
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
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "+");
    }

    #[test]
    fn parses_a_colon_ending_operator_as_a_value_name() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val :: = 2",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::ColonOp, 4, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::IntegerLiteral, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a value definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected ValDef");
        };
        let name_id = definition.name.as_name().text();
        assert!(matches!(
            parser.ast().get(*definition.rhs.as_ref().unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert_eq!(names.resolve(name_id), "::");
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

    #[test]
    fn parses_a_method_definition_with_an_expression_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::IntegerLiteral, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(parser.names.resolve(definition.name.as_name().text()), "f");
        assert!(definition.type_params.is_empty());
        assert!(definition.value_param_clauses.is_empty());
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::TypeTree(_)
        ));
        assert!(matches!(
            parser.ast().get(definition.rhs.unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_secondary_constructor_with_parameters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def this(x: Int) = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Keyword(HardKeyword::This), 4, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::IntegerLiteral, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser
            .with_secondary_constructor_allowed(true, |parser| {
                parser.parse_method_definition(Location::InBlock)
            })
        else {
            panic!("expected a secondary constructor definition");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(
            parser.names.resolve(definition.name.as_name().text()),
            "<init>"
        );
        assert_eq!(definition.value_param_clauses.len(), 1);
        assert_eq!(definition.value_param_clauses[0].len(), 1);
        let TreeKind::ValDef(parameter) =
            &parser.ast().get(definition.value_param_clauses[0][0]).kind
        else {
            panic!("secondary constructor parameter should be ValDef");
        };
        assert_eq!(parser.names.resolve(parameter.name.as_name().text()), "x");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_secondary_constructor_outside_a_template() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "def this(x: Int) = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Keyword(HardKeyword::This), 4, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 8, 9),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::IntegerLiteral, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        )
        .compilation_unit();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
    }

    #[test]
    fn method_definitions_are_block_stats_and_final_unit_is_synthetic() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "def f = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
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
            TreeKind::DefDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.expr).kind,
            TreeKind::Literal(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn local_method_before_a_final_expression_preserves_source_order() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "def f = 1\nf",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::IntegerLiteral, 8, 9),
                token(TokenKind::Newline, 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Eof, 11, 11),
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
            TreeKind::DefDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn preserves_method_parameter_clause_boundaries() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f(x: A, y: B)(z: C): D = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::Comma),
                    10,
                    11,
                ),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::ColonFollow, 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    16,
                    17,
                ),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    17,
                    18,
                ),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::ColonFollow, 19, 20),
                token(TokenKind::Identifier, 21, 22),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    22,
                    23,
                ),
                token(TokenKind::ColonFollow, 23, 24),
                token(TokenKind::Identifier, 25, 26),
                token(TokenKind::Operator, 27, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(definition.value_param_clauses.len(), 2);
        assert_eq!(definition.value_param_clauses[0].len(), 2);
        assert_eq!(definition.value_param_clauses[1].len(), 1);
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(matches!(
            parser.ast().get(definition.rhs.unwrap()).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reuses_type_parameter_parsing_for_methods() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def id[A](x: A): A = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 6),
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
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    9,
                    10,
                ),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    14,
                    15,
                ),
                token(TokenKind::ColonFollow, 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::Identifier, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(definition.type_params.len(), 1);
        assert_eq!(definition.value_param_clauses.len(), 1);
        assert!(matches!(
            parser.ast().get(definition.type_params[0]).kind,
            TreeKind::TypeDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_newlines_before_method_type_and_term_parameter_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def id\n[A]\n(x: A): A = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 6),
                token(TokenKind::Newline, 6, 7),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    7,
                    8,
                ),
                token(TokenKind::Identifier, 8, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    9,
                    10,
                ),
                token(TokenKind::Newline, 10, 11),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    11,
                    12,
                ),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::ColonFollow, 13, 14),
                token(TokenKind::Identifier, 15, 16),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    16,
                    17,
                ),
                token(TokenKind::ColonFollow, 17, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Operator, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Eof, 24, 24),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(definition.type_params.len(), 1);
        assert_eq!(definition.value_param_clauses.len(), 1);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn accepts_newlines_between_method_term_parameter_clauses() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f(x: A)\n(y: B): B = y",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    10,
                    11,
                ),
                token(TokenKind::Newline, 11, 12),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    12,
                    13,
                ),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::ColonFollow, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    17,
                    18,
                ),
                token(TokenKind::ColonFollow, 18, 19),
                token(TokenKind::Identifier, 20, 21),
                token(TokenKind::Operator, 22, 23),
                token(TokenKind::Identifier, 24, 25),
                token(TokenKind::Eof, 25, 25),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(definition.value_param_clauses.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_interleaved_type_parameter_clauses_without_reordering() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f[A](x: A)[B](y: B) = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    7,
                    8,
                ),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    8,
                    9,
                ),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    13,
                    14,
                ),
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
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    17,
                    18,
                ),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::ColonFollow, 19, 20),
                token(TokenKind::Identifier, 21, 22),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    22,
                    23,
                ),
                token(TokenKind::Operator, 24, 25),
                token(TokenKind::Identifier, 26, 27),
                token(TokenKind::Eof, 27, 27),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(definition.type_params.len(), 1);
        assert_eq!(definition.value_param_clauses.len(), 1);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::UnsupportedSyntax)
        );
    }

    #[test]
    fn reports_interleaved_type_parameters_after_a_newline_as_unsupported() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f[A](x: A)\n[B](y: B) = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    7,
                    8,
                ),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    8,
                    9,
                ),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    13,
                    14,
                ),
                token(TokenKind::Newline, 14, 15),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftBracket),
                    15,
                    16,
                ),
                token(TokenKind::Identifier, 16, 17),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightBracket),
                    17,
                    18,
                ),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    18,
                    19,
                ),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::ColonFollow, 20, 21),
                token(TokenKind::Identifier, 22, 23),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    23,
                    24,
                ),
                token(TokenKind::Operator, 25, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        assert!(matches!(parser.ast().get(id).kind, TreeKind::DefDef(_)));
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::UnsupportedSyntax })
        );
        assert!(
            !parser
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.kind() == ParseDiagnosticKind::ExpectedToken })
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_method_using_clause_with_given_parameter_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f(using ctx: Ctx) = body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Identifier, 12, 15),
                token(TokenKind::ColonFollow, 15, 16),
                token(TokenKind::Identifier, 17, 20),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    20,
                    21,
                ),
                token(TokenKind::Operator, 22, 23),
                token(TokenKind::Identifier, 24, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        let TreeKind::ValDef(ref parameter) =
            parser.ast().get(definition.value_param_clauses[0][0]).kind
        else {
            panic!("expected parameter ValDef");
        };
        assert_eq!(
            parameter.metadata.modifiers,
            vec![Modifier::Given, Modifier::Param]
        );
        assert!(matches!(
            parser.ast().get(definition.rhs.unwrap()).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_method_anonymous_using_clause_with_a_synthetic_parameter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f(using Ctx) = body",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 11),
                token(TokenKind::Identifier, 12, 15),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    15,
                    16,
                ),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::Identifier, 19, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        let [parameter] = definition.value_param_clauses[0].as_slice() else {
            panic!("expected one using parameter");
        };
        let TreeKind::ValDef(parameter) = &parser.ast().get(*parameter).kind else {
            panic!("expected parameter ValDef");
        };
        assert_eq!(parser.names.resolve(parameter.name.as_name().text()), "x$1");
        assert!(parameter.metadata.modifiers.contains(&Modifier::Given));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_a_method_default_parameter_value() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f(x: A = default): B = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Operator, 11, 12),
                token(TokenKind::Identifier, 13, 20),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    20,
                    21,
                ),
                token(TokenKind::ColonFollow, 21, 22),
                token(TokenKind::Identifier, 23, 24),
                token(TokenKind::Operator, 25, 26),
                token(TokenKind::Identifier, 27, 28),
                token(TokenKind::Eof, 28, 28),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        let TreeKind::ValDef(ref parameter) =
            parser.ast().get(definition.value_param_clauses[0][0]).kind
        else {
            panic!("expected parameter ValDef");
        };
        assert!(matches!(
            parameter.rhs,
            Some(rhs) if matches!(parser.ast().get(rhs).kind, TreeKind::Ident(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_single_indented_method_body_as_its_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f =\n  1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::Indent, 10, 10),
                token(TokenKind::IntegerLiteral, 10, 11),
                token(TokenKind::Outdent, 11, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert!(matches!(
            parser.ast().get(definition.rhs.unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_local_value_definitions_inside_an_indented_method_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f =\n  val y = 1\n  y",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Newline, 7, 8),
                token(TokenKind::Indent, 10, 10),
                token(TokenKind::Keyword(HardKeyword::Val), 10, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::IntegerLiteral, 18, 19),
                token(TokenKind::Newline, 19, 20),
                token(TokenKind::Identifier, 22, 23),
                token(TokenKind::Outdent, 23, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        let TreeKind::Block(ref body) = parser.ast().get(definition.rhs.unwrap()).kind else {
            panic!("expected a block method body");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(body.stats[0]).kind,
            TreeKind::ValDef(_)
        ));
        assert!(matches!(
            parser.ast().get(body.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_local_method_definitions_inside_an_indented_method_body() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def outer =\n  def inner = 1\n  inner",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Newline, 11, 12),
                token(TokenKind::Indent, 14, 14),
                token(TokenKind::Keyword(HardKeyword::Def), 14, 17),
                token(TokenKind::Identifier, 18, 23),
                token(TokenKind::Operator, 24, 25),
                token(TokenKind::IntegerLiteral, 26, 27),
                token(TokenKind::Newline, 27, 28),
                token(TokenKind::Identifier, 30, 35),
                token(TokenKind::Outdent, 35, 35),
                token(TokenKind::Eof, 35, 35),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        let TreeKind::Block(ref body) = parser.ast().get(definition.rhs.unwrap()).kind else {
            panic!("expected a block method body");
        };
        assert_eq!(body.stats.len(), 1);
        assert!(matches!(
            parser.ast().get(body.stats[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(matches!(
            parser.ast().get(body.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn recovers_from_a_missing_method_parameter_closer() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "def f(x: A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        )
        .compilation_unit();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Block(_)
        ));
    }

    #[test]
    fn recovers_from_a_missing_method_rhs() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "def f(x: A): B =",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    10,
                    11,
                ),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        )
        .compilation_unit();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Block(_)
        ));
    }

    #[test]
    fn malformed_method_signature_does_not_swallow_a_following_definition() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "def f(x A)\ndef g = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    9,
                    10,
                ),
                token(TokenKind::Newline, 10, 11),
                token(TokenKind::Keyword(HardKeyword::Def), 11, 14),
                token(TokenKind::Identifier, 15, 16),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::IntegerLiteral, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        )
        .compilation_unit();

        let TreeKind::Block(block) = &result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(block.stats.len(), 2);
        assert!(matches!(
            result.ast.get(block.stats[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(matches!(
            result.ast.get(block.stats[1]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn method_and_parameter_spans_cover_only_their_source_ranges() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f(x: A): B = x",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    5,
                    6,
                ),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::ColonFollow, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    10,
                    11,
                ),
                token(TokenKind::ColonFollow, 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 18).unwrap()
        );
        assert_eq!(
            parser
                .ast()
                .get(definition.value_param_clauses[0][0])
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(6, 10).unwrap()
        );
        assert_eq!(
            parser
                .ast()
                .get(definition.rhs.unwrap())
                .position
                .unwrap()
                .span()
                .range(),
            TextRange::new(17, 18).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_method_declaration_without_a_rhs_when_typed() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "def f: A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Def), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::ColonFollow, 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_method_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::DefDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected DefDef");
        };
        assert!(definition.rhs.is_none());
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_tuple_pattern_definition_as_a_pat_def() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val (a, b) = pair",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    4,
                    5,
                ),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    9,
                    10,
                ),
                token(TokenKind::Operator, 11, 12),
                token(TokenKind::Identifier, 13, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert_eq!(definition.patterns.len(), 1);
        assert!(matches!(
            parser.ast().get(definition.patterns[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(definition.rhs.is_some());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_an_extractor_pattern_definition_as_a_source_apply() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val Some(x) = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 8),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    8,
                    9,
                ),
                token(TokenKind::Identifier, 9, 10),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    10,
                    11,
                ),
                token(TokenKind::Operator, 12, 13),
                token(TokenKind::Identifier, 14, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert!(matches!(
            parser.ast().get(definition.patterns[0]).kind,
            TreeKind::Apply(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_binder_pattern_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x @ Some(y) = value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    12,
                    13,
                ),
                token(TokenKind::Identifier, 13, 14),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    14,
                    15,
                ),
                token(TokenKind::Operator, 16, 17),
                token(TokenKind::Identifier, 18, 23),
                token(TokenKind::Eof, 23, 23),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert!(matches!(
            parser.ast().get(definition.patterns[0]).kind,
            TreeKind::Bind(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_infix_pattern_definition_with_a_right_associative_operator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val head :: tail = xs",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 8),
                token(TokenKind::Operator, 9, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Operator, 17, 18),
                token(TokenKind::Identifier, 19, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert!(matches!(
            parser.ast().get(definition.patterns[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_pattern_definition_without_an_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val (a, b)",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    4,
                    5,
                ),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    9,
                    10,
                ),
                token(TokenKind::Eof, 10, 10),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert!(matches!(
            parser.ast().get(*definition.rhs.as_ref().unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
    }

    #[test]
    fn allows_a_typed_multiple_identifier_val_definition_without_an_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x, y: Value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonFollow, 8, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert_eq!(definition.patterns.len(), 2);
        assert!(definition.rhs.is_none());
        assert!(matches!(
            parser.ast().get(definition.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn allows_a_typed_multiple_identifier_var_definition_without_an_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "var x, y: Value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Var), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 8),
                token(TokenKind::ColonFollow, 8, 9),
                token(TokenKind::Identifier, 10, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert_eq!(definition.patterns.len(), 2);
        assert!(definition.rhs.is_none());
        assert_eq!(definition.modifiers.modifiers, vec![Modifier::Var]);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_a_typed_complex_pattern_definition_without_an_rhs() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val (x, y): Value",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    4,
                    5,
                ),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    9,
                    10,
                ),
                token(TokenKind::ColonFollow, 10, 11),
                token(TokenKind::Identifier, 12, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_value_definition(Location::Elsewhere)
        else {
            panic!("expected a definition statement");
        };
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &parser.ast().get(id).kind
        else {
            panic!("expected PatDef");
        };
        assert!(matches!(
            parser.ast().get(*definition.rhs.as_ref().unwrap()).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        );
    }

    #[test]
    fn rejects_a_non_identifier_after_a_comma_in_a_value_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x, (y, z) = pair",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 5, 6),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    7,
                    8,
                ),
                token(TokenKind::Identifier, 8, 9),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::Comma),
                    9,
                    10,
                ),
                token(TokenKind::Identifier, 11, 12),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    12,
                    13,
                ),
                token(TokenKind::Operator, 14, 15),
                token(TokenKind::Identifier, 16, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let _ = parser.parse_value_definition(Location::Elsewhere);

        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedPattern)
        );
    }

    #[test]
    fn rejects_an_extractor_after_a_comma_in_a_value_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "val x, Some(y) = pair",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 11),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    11,
                    12,
                ),
                token(TokenKind::Identifier, 12, 13),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::RightParen),
                    13,
                    14,
                ),
                token(TokenKind::Operator, 15, 16),
                token(TokenKind::Identifier, 17, 21),
                token(TokenKind::Eof, 21, 21),
            ],
            &mut names,
        );

        let _ = parser.parse_value_definition(Location::Elsewhere);

        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.kind() == ParseDiagnosticKind::ExpectedPattern)
        );
    }

    #[test]
    fn missing_definition_rhs_reports_a_diagnostic_and_reaches_eof() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "val x =",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        )
        .compilation_unit();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Block(_)
        ));
    }

    #[test]
    fn a_missing_definition_name_does_not_swallow_the_following_statement() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "val\ny",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        )
        .compilation_unit();

        let TreeKind::Block(block) = &result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(block.stats.len(), 1);
        assert!(matches!(
            result.ast.get(block.expr).kind,
            TreeKind::Ident(_)
        ));
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn an_unclosed_pattern_definition_keeps_recovery_bounded() {
        let mut names = NameInterner::new();
        let result = parser_for(
            "val (x,",
            vec![
                token(TokenKind::Keyword(HardKeyword::Val), 0, 3),
                token(
                    TokenKind::Punctuation(dotty_core::Punctuation::LeftParen),
                    4,
                    5,
                ),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(dotty_core::Punctuation::Comma), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        )
        .compilation_unit();

        assert!(!result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Block(_)
        ));
    }
}
