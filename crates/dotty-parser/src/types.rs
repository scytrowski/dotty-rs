use dotty_core::{Punctuation, TokenKind, TreeId, Untyped};

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
            args.push(self.simple_type());
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
    use dotty_core::{NameInterner, Punctuation, TextRange, TreeKind};

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
        assert!(parser.diagnostics().is_empty());
    }
}
