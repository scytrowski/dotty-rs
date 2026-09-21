use dotty_core::ast::{Apply, ApplyKind, NamedArg};
use dotty_core::{Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::Parser;

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

        self.observe_indented();
        self.advance();
        let argument = if self.cursor.at(TokenKind::Indent) {
            self.parse_feedback_indented_block()
        } else {
            self.expr()
        };
        self.alloc_from(
            crate::Mark { start },
            TreeKind::Apply(Apply {
                function,
                args: vec![argument],
                kind: ApplyKind::Regular,
            }),
        )
    }

    pub(super) fn parse_application(
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
            self.report(
                crate::ParseDiagnosticKind::ExpectedExpression,
                "expected an argument after `using`",
            );
        }
        let mut args = Vec::new();
        if !self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            loop {
                args.push(self.argument_expr());
                if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                    self.expect(TokenKind::Punctuation(Punctuation::RightParen));
                    break;
                }
                if self
                    .cursor
                    .at(TokenKind::Punctuation(Punctuation::RightParen))
                {
                    self.report(
                        crate::ParseDiagnosticKind::ExpectedExpression,
                        "expected an argument after `,`",
                    );
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
