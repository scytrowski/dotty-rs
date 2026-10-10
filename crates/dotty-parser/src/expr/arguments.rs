use dotty_core::ast::{Apply, ApplyKind, NamedArg};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{ExpressionIssue, ParseIssue, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(super) fn parse_colon_argument(&mut self, function: TreeId<Untyped>) -> TreeId<Untyped> {
        let start = self
            .ast
            .get(function)
            .position
            .map(|position| position.span().range().start())
            .unwrap_or_else(|| self.mark().start());

        let argument = self.parse_colon_argument_body();
        let application = self.alloc_from(
            crate::Mark { start },
            TreeKind::Apply(Apply {
                function,
                args: vec![argument],
                kind: ApplyKind::Regular,
            }),
        );

        // Colon arguments are a simple-expression suffix in Scala's grammar.
        // The case/block body may close an indentation region while the
        // enclosing expression still has selectors or applications to parse.
        let mark = crate::Mark {
            start: self
                .ast
                .get(function)
                .position
                .map(|position| position.span().range().start())
                .unwrap_or(start),
        };
        self.simple_expr_rest(mark, application, true)
    }

    pub(super) fn parse_colon_argument_body(&mut self) -> TreeId<Untyped> {
        if self.cursor.lookahead(1).kind == TokenKind::Keyword(dotty_core::HardKeyword::Case)
            || (self.cursor.lookahead(1).kind == TokenKind::Indent
                && self.cursor.lookahead(2).kind
                    == TokenKind::Keyword(dotty_core::HardKeyword::Case))
        {
            return self.parse_colon_case_argument();
        }
        self.observe_indented();
        self.advance();
        if self.cursor.at(TokenKind::Indent) {
            self.parse_feedback_indented_block()
        } else {
            self.expr()
        }
    }

    pub(crate) fn parse_application(
        &mut self,
        mark: crate::Mark,
        function: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        self.advance();
        let function = if matches!(self.ast.get(function).kind, TreeKind::New(_)) {
            self.constructor_select(function)
        } else {
            function
        };
        let kind = if self.current_is_using_marker() {
            self.advance();
            ApplyKind::Using
        } else {
            ApplyKind::Regular
        };
        if kind == ApplyKind::Using
            && self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::RightParen))
        {
            self.report_issue(ParseIssue::Expression(
                ExpressionIssue::MissingUsingArgument {
                    found: self.current().kind,
                },
            ));
        }
        let mut args = Vec::new();
        if !self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            loop {
                if self.at_enum_body_parent_boundary()
                    || matches!(
                        self.current().kind,
                        TokenKind::Punctuation(Punctuation::RightBrace)
                            | TokenKind::Outdent
                            | TokenKind::Eof
                    )
                {
                    self.report_issue(ParseIssue::Expression(ExpressionIssue::MissingArgument {
                        found: self.current().kind,
                    }));
                    args.push(self.error_expr(self.current_span()));
                } else {
                    args.push(self.argument_expr());
                }
                self.consume_newlines_before_argument_delimiter();
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.advance();
                    break;
                }
            }
        }
        self.alloc_from(
            mark,
            TreeKind::Apply(Apply {
                function,
                args,
                kind,
            }),
        )
    }

    fn current_is_using_marker(&mut self) -> bool {
        self.current().kind == TokenKind::Identifier
            && self
                .current_is_known_name(self.known_names().using)
                .unwrap_or(false)
    }

    fn consume_newlines_before_argument_delimiter(&mut self) {
        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }

        if offset == 0
            || !matches!(
                self.cursor.lookahead(offset).kind,
                TokenKind::Punctuation(Punctuation::Comma | Punctuation::RightParen)
            )
        {
            return;
        }

        for _ in 0..offset {
            self.advance();
        }
    }

    fn argument_expr(&mut self) -> TreeId<Untyped> {
        let tree = self.with_location(crate::Location::InArgs, |parser| parser.expr());
        self.normalize_named_argument(tree)
    }

    fn normalize_named_argument(&mut self, tree: TreeId<Untyped>) -> TreeId<Untyped> {
        let TreeKind::Assign(assignment) = self.ast.get(tree).kind else {
            return tree;
        };
        let TreeKind::Ident(identifier) = self.ast.get(assignment.lhs).kind else {
            return tree;
        };

        self.alloc(
            TreeKind::NamedArg(NamedArg {
                name: identifier.name,
                arg: assignment.rhs,
            }),
            self.ast.get(tree).position,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::NameInterner;
    use dotty_core::ast::UntypedNode;

    #[test]
    fn parses_a_final_argument_splice_as_typed_wildcard_star() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(args*)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        let [argument] = application.args.as_slice() else {
            panic!("expected one argument");
        };
        let TreeKind::Typed(typed) = &parser.ast().get(*argument).kind else {
            panic!("expected a typed vararg splice");
        };
        let TreeKind::Ident(wildcard_star) = &parser.ast().get(typed.tpt).kind else {
            panic!("expected the wildcard-star type marker");
        };
        assert_eq!(parser.names.resolve(wildcard_star.name.text()), "_*");
        assert_eq!(
            parser.ast().get(typed.tpt).position.unwrap().span().range(),
            dotty_core::TextRange::new(6, 7).unwrap()
        );
        assert_eq!(
            parser.ast().get(*argument).position.unwrap().span().range(),
            dotty_core::TextRange::new(2, 7).unwrap()
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_legacy_wildcard_splice_ascription_in_final_argument_position() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(args: _*)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        let [argument] = application.args.as_slice() else {
            panic!("expected one argument");
        };
        let TreeKind::Typed(typed) = &parser.ast().get(*argument).kind else {
            panic!("expected a typed wildcard-star splice");
        };
        let TreeKind::Ident(marker) = &parser.ast().get(typed.tpt).kind else {
            panic!("expected the wildcard-star marker");
        };
        assert_eq!(parser.names.resolve(marker.name.text()), "_*");
        assert_eq!(
            parser.ast().get(typed.tpt).position.unwrap().span().range(),
            dotty_core::TextRange::new(8, 10).unwrap()
        );
        assert_eq!(
            parser.ast().get(*argument).position.unwrap().span().range(),
            dotty_core::TextRange::new(2, 10).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn diagnoses_legacy_wildcard_splice_ascriptions_outside_final_argument_position() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(args: _*, tail)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::ColonFollow, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 9, 10),
                token(TokenKind::Punctuation(Punctuation::Comma), 10, 11),
                token(TokenKind::Identifier, 12, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        assert_eq!(application.args.len(), 2);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::Typed(_)
        ));
        assert!(matches!(
            parser.ast().get(application.args[1]).kind,
            TreeKind::Ident(_)
        ));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            crate::ParseDiagnosticKind::UnexpectedToken
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn diagnoses_legacy_wildcard_splice_ascription_outside_an_argument_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "args: _*",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::ColonFollow, 4, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(parser.ast().get(tree).kind, TreeKind::Typed(_)));
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(
            parser.diagnostics()[0].kind(),
            crate::ParseDiagnosticKind::UnexpectedToken
        );
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_splice_after_earlier_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(head, tail*)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 6),
                token(TokenKind::Punctuation(Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 12),
                token(TokenKind::Operator, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        assert_eq!(application.args.len(), 2);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(application.args[1]).kind,
            TreeKind::Typed(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn keeps_multiplication_in_an_argument_as_an_infix_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(a * b)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Operator, 4, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::RightParen), 7, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn consumes_newline_before_a_closing_argument_delimiter() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(a\n)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightParen), 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let tree = parser.expr();

        assert!(matches!(parser.ast().get(tree).kind, TreeKind::Apply(_)));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn keeps_newline_before_a_non_delimiter_significant_in_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(a\nb)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        parser.expr();

        assert!(parser.diagnostics().iter().any(|diagnostic| {
            matches!(
                diagnostic.issue(),
                crate::ParseIssue::ExpectedToken {
                    expected: TokenKind::Punctuation(Punctuation::RightParen),
                    found: TokenKind::Newline,
                }
            )
        }));
        assert_eq!(parser.current().kind, TokenKind::Newline);
    }

    #[test]
    fn accepts_a_trailing_comma_after_an_application_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(a,)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightParen), 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        assert_eq!(application.args.len(), 1);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn missing_argument_before_a_nonfinal_comma_is_still_diagnosed() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(a,,b)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Punctuation(Punctuation::Comma), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let tree = parser.expr();
        assert!(matches!(parser.ast().get(tree).kind, TreeKind::Apply(_)));
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic.kind() == crate::ParseDiagnosticKind::ExpectedExpression
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn diagnoses_a_nonfinal_splice_without_consuming_the_next_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "f(values*, other)",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 8),
                token(TokenKind::Operator, 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 16),
                token(TokenKind::Punctuation(Punctuation::RightParen), 16, 17),
                token(TokenKind::Eof, 17, 17),
            ],
            &mut names,
        );

        let tree = parser.expr();
        let TreeKind::Apply(application) = &parser.ast().get(tree).kind else {
            panic!("expected an application");
        };
        assert_eq!(application.args.len(), 2);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(application.args[1]).kind,
            TreeKind::Ident(_)
        ));
        assert!(parser.diagnostics().iter().any(|diagnostic| {
            diagnostic.issue() == &ParseIssue::Expression(ExpressionIssue::NonFinalArgumentSpread)
        }));
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }
}
