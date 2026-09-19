use dotty_core::ast::{Block, Literal};
use dotty_core::{
    AstArena, Constant, HardKeyword, Punctuation, SourceId, SourceText, TokenKind, TokenSource,
    TreeId, TreeKind, Untyped,
};

use crate::{Location, ParseDiagnostic, ParseDiagnosticKind, Parser};

/// Result of parsing one source compilation unit.
#[derive(Debug)]
pub struct ParseResult {
    pub ast: AstArena<Untyped>,
    pub root: TreeId<Untyped>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

/// Parses a source-backed token stream as a Scala 3.9.0 compilation unit.
pub fn parse_compilation_unit<S: TokenSource>(
    source: SourceText<'_>,
    source_id: SourceId,
    tokens: S,
    names: &mut dotty_core::NameInterner,
) -> ParseResult {
    Parser::new(source, source_id, tokens, names).compilation_unit()
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses the supported expression sequence into a stable synthetic block root.
    pub fn compilation_unit(mut self) -> ParseResult {
        let unit_mark = self.mark();
        let mut trees = Vec::new();

        while self.current().kind != TokenKind::Eof {
            self.consume_statement_separators();
            if self.current().kind == TokenKind::Eof {
                break;
            }

            let checkpoint = self.cursor.checkpoint();
            let tree = if is_unsupported_start(self.current().kind) {
                self.parse_unsupported_syntax()
            } else {
                self.with_location(Location::Elsewhere, |parser| parser.expr())
            };
            trees.push(tree);

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a compilation unit",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if self.current().kind != TokenKind::Eof && !is_statement_separator(self.current().kind)
            {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "expected a statement separator",
                );
                self.recover_until(crate::RecoverySet::Statement);
            }
        }

        let (stats, expr) = match trees.pop() {
            Some(expr) => (trees, expr),
            None => {
                let expr = self.alloc(
                    TreeKind::Literal(Literal {
                        value: Constant::Unit,
                    }),
                    Some(self.current_span()),
                );
                (Vec::new(), expr)
            }
        };
        let root = self.alloc(
            TreeKind::Block(Block { stats, expr }),
            Some(self.span_from(unit_mark)),
        );

        ParseResult {
            ast: self.ast,
            root,
            diagnostics: self.diagnostics,
        }
    }

    fn consume_statement_separators(&mut self) {
        while is_statement_separator(self.current().kind) {
            self.advance();
        }
    }

    fn parse_unsupported_syntax(&mut self) -> TreeId<Untyped> {
        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            format!(
                "syntax beginning with {:?} is not supported by this parser milestone",
                self.current().kind
            ),
        );
        self.advance();
        self.recover_until(crate::RecoverySet::Statement);
        self.error_expr(position)
    }
}

const fn is_statement_separator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Punctuation(Punctuation::Semicolon)
            | TokenKind::Outdent
    )
}

const fn is_unsupported_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            HardKeyword::Class
                | HardKeyword::Def
                | HardKeyword::For
                | HardKeyword::Try
                | HardKeyword::Match
                | HardKeyword::Val
                | HardKeyword::Var
                | HardKeyword::Type
                | HardKeyword::Object
                | HardKeyword::Trait
                | HardKeyword::Enum
                | HardKeyword::Given
        )
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dotty_core::ast::UntypedNode;
    use dotty_core::{
        HardKeyword, NameInterner, SourceId, SourceText, TextRange, Token, TokenSource, TokenValue,
    };

    pub(crate) struct VecTokenSource {
        tokens: Vec<Token>,
        index: usize,
    }

    impl TokenSource for VecTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index]
        }

        fn position(&self) -> usize {
            self.index
        }

        fn advance(&mut self) {
            if self.index + 1 < self.tokens.len() {
                self.index += 1;
            }
        }

        fn lookahead(&mut self, n: usize) -> &Token {
            let index = self
                .index
                .saturating_add(n)
                .min(self.tokens.len().saturating_sub(1));
            &self.tokens[index]
        }

        fn observe(&mut self, _event: dotty_core::ScannerEvent) {}
    }

    pub(crate) fn token(kind: TokenKind, start: u32, end: u32) -> Token {
        Token {
            kind,
            span: TextRange::new(start, end).expect("valid test range"),
            value: TokenValue::None,
        }
    }

    pub(crate) fn parser_for<'src, 'names>(
        source: &'src str,
        tokens: Vec<Token>,
        names: &'names mut NameInterner,
    ) -> Parser<'src, 'names, VecTokenSource> {
        Parser::new(
            SourceText::new(source).expect("valid source"),
            SourceId::from_index(1),
            VecTokenSource { tokens, index: 0 },
            names,
        )
    }

    #[test]
    fn compilation_unit_uses_a_stable_block_root_for_one_expression() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert!(result.diagnostics.is_empty());
        let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert!(stats.is_empty());
        assert!(matches!(result.ast.get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn empty_compilation_unit_uses_a_synthetic_unit_expression() {
        let mut names = NameInterner::new();
        let parser = parser_for("", vec![token(TokenKind::Eof, 0, 0)], &mut names);

        let result = parser.compilation_unit();

        assert!(result.diagnostics.is_empty());
        let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert!(stats.is_empty());
        assert!(matches!(
            result.ast.get(expr).kind,
            TreeKind::Literal(Literal {
                value: Constant::Unit
            })
        ));
        assert_eq!(
            result.ast.get(expr).position.unwrap().span().range(),
            TextRange::new(0, 0).unwrap()
        );
    }

    #[test]
    fn whitespace_only_compilation_unit_uses_a_synthetic_unit_expression() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "\n",
            vec![token(TokenKind::Newline, 0, 1), token(TokenKind::Eof, 1, 1)],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert!(result.diagnostics.is_empty());
        let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert!(stats.is_empty());
        assert!(matches!(
            result.ast.get(expr).kind,
            TreeKind::Literal(Literal {
                value: Constant::Unit
            })
        ));
        assert_eq!(
            result.ast.get(expr).position.unwrap().span().range(),
            TextRange::new(1, 1).unwrap()
        );
    }

    #[test]
    fn compilation_unit_preserves_multiple_expressions_across_newline() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "a\nb",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(stats.len(), 1);
        assert!(matches!(result.ast.get(stats[0]).kind, TreeKind::Ident(_)));
        assert!(matches!(result.ast.get(expr).kind, TreeKind::Ident(_)));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn malformed_input_recovers_and_preserves_a_following_valid_expression() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "@\nx",
            vec![
                token(TokenKind::Error, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(stats.len(), 1);
        assert!(matches!(
            result.ast.get(stats[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert!(matches!(result.ast.get(expr).kind, TreeKind::Ident(_)));
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::ExpectedExpression
        );
    }

    #[test]
    fn unsupported_valid_syntax_gets_an_unsupported_diagnostic() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "class A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Class), 0, 5),
                token(TokenKind::Identifier, 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Block(Block { .. })
        ));
    }

    #[test]
    fn public_free_function_constructs_the_same_compilation_unit_result() {
        let mut names = NameInterner::new();
        let result = parse_compilation_unit(
            SourceText::new("x").expect("valid source"),
            SourceId::from_index(2),
            VecTokenSource {
                tokens: vec![
                    token(TokenKind::Identifier, 0, 1),
                    token(TokenKind::Eof, 1, 1),
                ],
                index: 0,
            },
            &mut names,
        );

        assert!(result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::Block(Block { .. })
        ));
    }
}
