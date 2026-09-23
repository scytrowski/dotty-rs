use dotty_core::ast::{Parens, UntypedNode};
use dotty_core::{Name, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::references::{QualifiedReferenceError, ReferenceNamespace};
use crate::{ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses a bracketed, comma-separated list of simple type arguments.
    ///
    /// The caller owns the tree that precedes the list; this helper is shared
    /// by term-level type applications and applied type trees so that their
    /// delimiter and recovery behavior stays identical.
    pub(crate) fn parse_type_argument_list(&mut self) -> Vec<TreeId<Untyped>> {
        self.expect(TokenKind::Punctuation(Punctuation::LeftBracket));

        let mut args = Vec::new();
        if self.accept(TokenKind::Punctuation(Punctuation::RightBracket)) {
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected a type argument between `[` and `]`",
            );
            return args;
        }

        loop {
            args.push(self.type_expr());
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
                break;
            }

            if self
                .cursor
                .at(TokenKind::Punctuation(Punctuation::RightBracket))
            {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type argument after `,`",
                );
                self.advance();
                break;
            }
        }
        args
    }

    /// Parses the currently supported infix type subset.
    ///
    /// `simple_type` deliberately remains an atomic/applied type parser. The
    /// higher-level entry owns the Scala union/intersection precedence.
    pub(crate) fn type_expr(&mut self) -> TreeId<Untyped> {
        self.parse_union_type()
    }

    fn parse_union_type(&mut self) -> TreeId<Untyped> {
        let mut tree = self.parse_intersection_type();
        while let Some(operator) = self.accept_type_infix_operator("|") {
            self.consume_type_infix_newlines();
            let right = self.parse_intersection_type();
            tree = self.alloc_infix(tree, operator, right);
        }
        tree
    }

    fn parse_intersection_type(&mut self) -> TreeId<Untyped> {
        let mut tree = self.parse_type_operand();
        while let Some(operator) = self.accept_type_infix_operator("&") {
            self.consume_type_infix_newlines();
            let right = self.parse_type_operand();
            tree = self.alloc_infix(tree, operator, right);
        }
        tree
    }

    fn parse_type_operand(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        match self.current().kind {
            TokenKind::Identifier | TokenKind::BackquotedIdentifier => {
                return self.simple_type();
            }
            TokenKind::Punctuation(Punctuation::LeftParen) => {
                return self.parse_parenthesized_type(mark);
            }
            _ => {}
        }

        let position = self.current_span();
        self.report(ParseDiagnosticKind::ExpectedType, "expected a type operand");
        self.error_type(position)
    }

    fn parse_parenthesized_type(&mut self, mark: crate::Mark) -> TreeId<Untyped> {
        self.advance();
        if self.accept(TokenKind::Punctuation(Punctuation::RightParen)) {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "an empty parenthesized type requires a function type",
            );
            return self.error_type(position);
        }

        let inner = self.type_expr();
        if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
            self.expect(TokenKind::Punctuation(Punctuation::RightParen));
            return self.alloc_from(
                mark,
                TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })),
            );
        }

        let mut elements = vec![inner];
        while !self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::RightParen))
            && self.current().kind != TokenKind::Eof
        {
            elements.push(self.type_expr());
            if !self.accept(TokenKind::Punctuation(Punctuation::Comma)) {
                break;
            }
        }
        self.expect(TokenKind::Punctuation(Punctuation::RightParen));
        self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(dotty_core::ast::Tuple { elements })),
        )
    }

    fn consume_type_infix_newlines(&mut self) {
        if !matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            return;
        }

        let mut offset = 0;
        while matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            offset += 1;
        }
        if !matches!(
            self.cursor.lookahead(offset).kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            return;
        }

        while offset > 0 {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            offset -= 1;
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn accept_type_infix_operator(&mut self, expected: &str) -> Option<Name> {
        if !matches!(
            self.current().kind,
            TokenKind::Operator | TokenKind::ColonOp
        ) || !self.current_text_is(expected)
        {
            return None;
        }

        let operator = *self.intern_current_type_name().ok()?.as_name();
        self.advance();
        Some(operator)
    }

    /// Parses the small simple-type subset needed by simple expressions.
    pub(crate) fn simple_type(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        let mut tree = self.simple_type_reference();

        while self
            .cursor
            .at(TokenKind::Punctuation(Punctuation::LeftBracket))
        {
            let args = self.parse_type_argument_list();
            tree = self.alloc_from(
                mark,
                dotty_core::TreeKind::AppliedTypeTree(dotty_core::ast::AppliedTypeTree {
                    tpt: tree,
                    args,
                }),
            );
        }
        tree
    }

    /// Parses only `id { '.' id }`, for grammar positions whose suffixes have
    /// a distinct meaning (for example the current `derives` production).
    pub(crate) fn simple_type_reference(&mut self) -> TreeId<Untyped> {
        match self.parse_qualified_reference(ReferenceNamespace::Type) {
            Ok(tree) => tree,
            Err(QualifiedReferenceError::MissingInitial) => {
                let position = self.current_span();
                self.report(ParseDiagnosticKind::ExpectedType, "expected a simple type");
                if !is_type_recovery_boundary(self.current().kind) {
                    self.advance();
                }
                self.error_type(position)
            }
            Err(QualifiedReferenceError::MissingSegment) => {
                self.report(
                    ParseDiagnosticKind::ExpectedType,
                    "expected a type name after `.`",
                );
                self.error_type(self.current_span())
            }
        }
    }
}

const fn is_type_recovery_boundary(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eof
            | TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Indent
            | TokenKind::Outdent
            | TokenKind::ColonFollow
            | TokenKind::ColonOp
            | TokenKind::ColonEol
            | TokenKind::Punctuation(
                Punctuation::Comma
                    | Punctuation::Colon
                    | Punctuation::LeftBrace
                    | Punctuation::RightBrace
                    | Punctuation::RightBracket
                    | Punctuation::RightParen
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TextRange, TreeKind};

    #[test]
    fn parses_a_parenthesized_type_as_parens() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::RightParen), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) =
            parser.ast().get(id).kind
        else {
            panic!("expected a parenthesized type");
        };
        assert!(matches!(parser.ast().get(inner).kind, TreeKind::Ident(_)));
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 3).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parenthesized_type_parses_a_full_inner_type_expression() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A | B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) =
            parser.ast().get(id).kind
        else {
            panic!("expected a parenthesized type");
        };
        assert!(matches!(
            parser.ast().get(inner).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn parses_a_two_element_tuple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A, B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightParen), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a tuple type");
        };
        assert_eq!(tuple.elements.len(), 2);
        assert!(
            tuple
                .elements
                .iter()
                .all(|element| matches!(parser.ast().get(*element).kind, TreeKind::Ident(_)))
        );
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 6).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
        assert_eq!(parser.current().kind, TokenKind::Eof);
    }

    #[test]
    fn tuple_type_elements_use_full_type_expressions() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A | B, C & D)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Operator, 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::Comma), 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Operator, 10, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Punctuation(Punctuation::RightParen), 13, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a tuple type");
        };
        assert_eq!(tuple.elements.len(), 2);
        assert!(tuple.elements.iter().all(|element| matches!(
            parser.ast().get(*element).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        )));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_nested_tuple_types_without_flattening_them() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "((A, B), C)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref outer)) = parser.ast().get(id).kind
        else {
            panic!("expected an outer tuple type");
        };
        assert!(matches!(
            parser.ast().get(outer.elements[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(outer.elements.len(), 2);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_tuple_types_inside_applied_type_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[(A, B)]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 5, 6),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Punctuation(Punctuation::Comma), 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightParen), 10, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(ref applied) = parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn rejects_an_empty_parenthesized_type_without_building_an_empty_tuple() {
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

        let id = parser.type_expr();
        assert!(!matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn recovers_an_unterminated_tuple_type_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A,",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Tuple(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedToken))
        );
    }

    #[test]
    fn recovers_a_missing_tuple_element_without_swallowing_the_closer() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(A,, B)",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftParen), 0, 1),
                token(TokenKind::Identifier, 1, 2),
                token(TokenKind::Punctuation(Punctuation::Comma), 2, 3),
                token(TokenKind::Punctuation(Punctuation::Comma), 3, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(UntypedNode::Tuple(ref tuple)) = parser.ast().get(id).kind
        else {
            panic!("expected a recovered tuple type");
        };
        assert_eq!(tuple.elements.len(), 3);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(
            parser
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic.kind(), ParseDiagnosticKind::ExpectedType))
        );
    }

    #[test]
    fn leaves_function_type_arrows_for_the_later_type_milestone() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "type F = (A, B) => C",
            vec![
                token(TokenKind::Keyword(HardKeyword::Type), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 9, 10),
                token(TokenKind::Identifier, 10, 11),
                token(TokenKind::Punctuation(Punctuation::Comma), 11, 12),
                token(TokenKind::Identifier, 13, 14),
                token(TokenKind::Punctuation(Punctuation::RightParen), 14, 15),
                token(TokenKind::Operator, 16, 18),
                token(TokenKind::Identifier, 19, 20),
                token(TokenKind::Eof, 20, 20),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn parses_a_simple_type_in_the_type_namespace() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Value",
            vec![
                token(TokenKind::Identifier, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Ident(ident) = parser.ast().get(id).kind else {
            panic!("expected type identifier");
        };

        assert!(ident.name.is_type());
        assert_eq!(
            parser.ast().get(id).position.unwrap().span().range(),
            TextRange::new(0, 5).unwrap()
        );
        let name = ident.name;
        drop(parser);
        assert_eq!(names.resolve(name.text()), "Value");
    }

    #[test]
    fn parses_a_union_type_with_a_type_namespace_operator() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A | B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(infix)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected a type infix tree");
        };
        assert_eq!(parser.names.resolve(infix.op.text()), "|");
        assert!(infix.op.is_type());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn intersection_binds_tighter_than_union() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A | B & C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer union");
        };
        assert_eq!(parser.names.resolve(outer.op.text()), "|");
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(inner)) =
            &parser.ast().get(outer.right).kind
        else {
            panic!("expected the nested intersection");
        };
        assert_eq!(parser.names.resolve(inner.op.text()), "&");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn same_precedence_type_operators_are_left_associative() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A & B & C",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(outer)) =
            &parser.ast().get(id).kind
        else {
            panic!("expected the outer intersection");
        };
        assert_eq!(parser.names.resolve(outer.op.text()), "&");
        assert!(matches!(
            parser.ast().get(outer.left).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_union_inside_type_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[A | B]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Identifier, 9, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(id).kind else {
            panic!("expected an applied type");
        };
        assert_eq!(applied.args.len(), 1);
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_nested_applied_types_with_an_intersection_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "Option[List[A | B]]",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 6, 7),
                token(TokenKind::Identifier, 7, 11),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 11, 12),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Operator, 14, 15),
                token(TokenKind::Identifier, 16, 17),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 17, 18),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let outer_id = parser.type_expr();
        let TreeKind::AppliedTypeTree(outer) = &parser.ast().get(outer_id).kind else {
            panic!("expected the outer applied type");
        };
        let inner_id = outer.args[0];
        let TreeKind::AppliedTypeTree(inner) = &parser.ast().get(inner_id).kind else {
            panic!("expected the nested applied type");
        };
        assert!(matches!(
            parser.ast().get(inner.args[0]).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn consumes_a_newline_after_a_type_operator_when_an_operand_follows() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A |\nB",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Newline, 3, 4),
                token(TokenKind::Identifier, 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_a_missing_type_operand_without_consuming_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A |",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.type_expr();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(_))
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn recovers_a_missing_type_operand_before_a_closing_bracket() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[A |]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Operator, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn leaves_unimplemented_type_operators_unconsumed() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "A || B",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Operator, 2, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        parser.type_expr();
        assert_eq!(parser.current().kind, TokenKind::Operator);
        assert_eq!(parser.current_text().unwrap(), "||");
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_qualified_simple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "pkg.Value",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::Identifier, 4, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected qualified type");
        };

        assert!(selection.name.is_type());
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_a_shared_type_argument_list_in_the_type_namespace() {
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

        let args = parser.parse_type_argument_list();
        assert_eq!(args.len(), 2);
        assert!(args.iter().all(|id| matches!(
            parser.ast().get(*id).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        )));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_applied_simple_type() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[Int]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(id).kind else {
            panic!("expected an applied type tree");
        };
        assert!(matches!(
            parser.ast().get(applied.tpt).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert_eq!(applied.args.len(), 1);
        assert!(matches!(
            parser.ast().get(applied.args[0]).kind,
            TreeKind::Ident(ident) if ident.name.is_type()
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_nested_and_repeated_applied_simple_types() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "F[A][B]",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 3, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let outer_id = parser.simple_type();
        let TreeKind::AppliedTypeTree(outer) = &parser.ast().get(outer_id).kind else {
            panic!("expected the outer applied type tree");
        };
        let inner_id = outer.tpt;
        let TreeKind::AppliedTypeTree(inner) = &parser.ast().get(inner_id).kind else {
            panic!("expected the inner applied type tree");
        };
        assert_eq!(inner.args.len(), 1);
        assert_eq!(outer.args.len(), 1);
        assert_eq!(
            parser.ast().get(outer_id).position.unwrap().span().range(),
            TextRange::new(0, 7).unwrap()
        );
        assert_eq!(
            parser.ast().get(inner_id).position.unwrap().span().range(),
            TextRange::new(0, 4).unwrap()
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn reports_an_empty_applied_type_argument_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 5, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::AppliedTypeTree(_)
        ));
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedType
        ));
    }

    #[test]
    fn recovers_a_missing_applied_type_argument_before_a_comma() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[, Int]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 10, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        let id = parser.simple_type();
        let TreeKind::AppliedTypeTree(applied) = &parser.ast().get(id).kind else {
            panic!("expected an applied type tree");
        };
        assert_eq!(applied.args.len(), 2);
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }

    #[test]
    fn recovers_a_missing_applied_type_closer_at_eof() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[Int",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Eof, 8, 8),
            ],
            &mut names,
        );

        parser.simple_type();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(matches!(
            parser.diagnostics()[0].kind(),
            ParseDiagnosticKind::ExpectedToken
        ));
    }

    #[test]
    fn recovers_repeated_commas_in_an_applied_type_argument_list() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "List[Int,, String]",
            vec![
                token(TokenKind::Identifier, 0, 4),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 4, 5),
                token(TokenKind::Identifier, 5, 8),
                token(TokenKind::Punctuation(Punctuation::Comma), 8, 9),
                token(TokenKind::Punctuation(Punctuation::Comma), 9, 10),
                token(TokenKind::Identifier, 11, 17),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        parser.simple_type();
        assert_eq!(parser.current().kind, TokenKind::Eof);
        assert_eq!(parser.diagnostics().len(), 1);
    }
}
