use dotty_core::ast::Block;
use dotty_core::{AstArena, SourceId, SourceText, TokenSource, TreeId, TreeKind, Untyped};

use crate::statements::StatementSequenceBoundary;
use crate::{ParseDiagnostic, Parser};

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
        let (stats, expr) =
            self.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        self.report_escaping_placeholders();
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
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ParseDiagnosticKind;
    use dotty_core::ast::{Literal, UntypedNode};
    use dotty_core::{
        Constant, HardKeyword, NameInterner, Punctuation, SourceId, SourceText, TextRange, Token,
        TokenKind, TokenSource, TokenValue,
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
    fn compilation_unit_reports_an_escaping_placeholder() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "_",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnboundPlaceholderParameter
        );
        assert_eq!(result.diagnostics[0].span(), TextRange::new(0, 0).unwrap());
    }

    #[test]
    fn placeholder_does_not_leak_to_a_following_compilation_unit_statement() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "_\nvalue",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Identifier, 2, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnboundPlaceholderParameter
        );
        let TreeKind::Block(Block { ref stats, expr }) = result.ast.get(result.root).kind else {
            panic!("expected block root");
        };
        assert_eq!(stats.len(), 1);
        assert!(matches!(result.ast.get(expr).kind, TreeKind::Ident(_)));
    }

    #[test]
    fn block_reports_an_escaping_placeholder() {
        let mut names = NameInterner::new();
        let parser = parser_for(
            "{ _; value }",
            vec![
                token(TokenKind::Punctuation(Punctuation::LeftBrace), 0, 1),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Punctuation(Punctuation::Semicolon), 3, 4),
                token(TokenKind::Identifier, 5, 10),
                token(TokenKind::Punctuation(Punctuation::RightBrace), 11, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        let result = parser.compilation_unit();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnboundPlaceholderParameter
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
            "enum A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Enum), 0, 4),
                token(TokenKind::Identifier, 5, 6),
                token(TokenKind::Eof, 6, 6),
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
