use dotty_core::ast::{Block, Ident, PackageDef};
use dotty_core::{AstArena, SourceId, SourceText, TokenSource, TreeId, TreeKind, Untyped};

use crate::statements::StatementSequenceBoundary;
use crate::{Location, ParseDiagnostic, ParseDiagnosticKind, ParseKind, Parser};

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
    Parser::new(source, source_id, tokens, names).source_compilation_unit()
}

/// Parses one source-backed expression fragment.
///
/// This entry point is intended for parser tooling and differential tests. It
/// uses the same expression grammar as nested parser contexts and requires the
/// input to end after that expression; it is not an alternate compilation-unit
/// dialect.
pub fn parse_expression_fragment<S: TokenSource>(
    source: SourceText<'_>,
    source_id: SourceId,
    tokens: S,
    names: &mut dotty_core::NameInterner,
) -> ParseResult {
    Parser::new(source, source_id, tokens, names).parse_expression_fragment()
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses one standalone expression for parser tooling and differential
    /// tests. This is a fragment entry, not a second parser dialect.
    pub fn parse_expression_fragment(mut self) -> ParseResult {
        let expression = self.with_parse_kind(ParseKind::Expr, |parser| {
            parser.with_location(Location::Elsewhere, |parser| parser.expr())
        });

        while matches!(
            self.current().kind,
            dotty_core::TokenKind::Newline | dotty_core::TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
        if self.current().kind != dotty_core::TokenKind::Eof {
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
                "expected end of expression fragment",
            );
            self.recover_until(crate::RecoverySet::Statement);
        }

        self.report_escaping_placeholders();
        ParseResult {
            ast: self.ast,
            root: expression,
            diagnostics: self.diagnostics,
        }
    }

    /// Parses the supported expression sequence into a synthetic block root.
    ///
    /// This lower-level helper remains useful for parser-internal block and
    /// definition tests. Call [`parse_compilation_unit`] for a source file.
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

    /// Parses one source compilation unit into its source-level package root.
    pub fn source_compilation_unit(mut self) -> ParseResult {
        let unit_mark = self.mark();
        let stats = self.parse_top_level_sequence(StatementSequenceBoundary::CompilationUnit);

        self.report_escaping_placeholders();
        let root =
            if stats.len() == 1 && matches!(self.ast.get(stats[0]).kind, TreeKind::PackageDef(_)) {
                stats[0]
            } else {
                let empty_name_id = self.names.intern("<empty>");
                let empty_name = self.alloc(
                    TreeKind::Ident(Ident {
                        name: *dotty_core::TermName::new(empty_name_id).as_name(),
                        backquoted: false,
                    }),
                    Some(self.zero_width_span(unit_mark.start())),
                );
                self.alloc(
                    TreeKind::PackageDef(PackageDef {
                        name: empty_name,
                        stats,
                    }),
                    Some(self.span_from(unit_mark)),
                )
            };

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
    use dotty_core::ast::{Block, Literal, UntypedNode};
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
    fn public_free_function_constructs_a_source_compilation_unit_root() {
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

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnsupportedSyntax
        );
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::PackageDef(PackageDef { .. })
        ));
    }

    #[test]
    fn source_compilation_unit_uses_an_empty_package_for_an_empty_file() {
        let mut names = NameInterner::new();
        let result = parse_compilation_unit(
            SourceText::new("").expect("valid source"),
            SourceId::from_index(5),
            VecTokenSource {
                tokens: vec![token(TokenKind::Eof, 0, 0)],
                index: 0,
            },
            &mut names,
        );

        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        assert!(package.stats.is_empty());
        assert!(result.diagnostics.is_empty());
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 0).unwrap()
        );
    }

    #[test]
    fn source_compilation_unit_keeps_an_explicit_package_as_the_root() {
        let mut names = NameInterner::new();
        let result = parse_compilation_unit(
            SourceText::new("package foo").expect("valid source"),
            SourceId::from_index(6),
            VecTokenSource {
                tokens: vec![
                    token(TokenKind::Keyword(HardKeyword::Package), 0, 7),
                    token(TokenKind::Identifier, 8, 11),
                    token(TokenKind::Eof, 11, 11),
                ],
                index: 0,
            },
            &mut names,
        );

        let TreeKind::PackageDef(package) = &result.ast.get(result.root).kind else {
            panic!("expected package root");
        };
        assert!(package.stats.is_empty());
        assert!(result.diagnostics.is_empty());
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 11).unwrap()
        );
    }

    #[test]
    fn expression_fragment_returns_the_expression_root_at_eof() {
        let mut names = NameInterner::new();
        let result = parse_expression_fragment(
            SourceText::new("a + b").expect("valid source"),
            SourceId::from_index(3),
            VecTokenSource {
                tokens: vec![
                    token(TokenKind::Identifier, 0, 1),
                    token(TokenKind::Operator, 2, 3),
                    token(TokenKind::Identifier, 4, 5),
                    token(TokenKind::Eof, 5, 5),
                ],
                index: 0,
            },
            &mut names,
        );

        assert!(result.diagnostics.is_empty());
        assert!(matches!(
            result.ast.get(result.root).kind,
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))
        ));
        assert_eq!(
            result.ast.get(result.root).position.unwrap().span().range(),
            TextRange::new(0, 5).unwrap()
        );
    }

    #[test]
    fn expression_fragment_reports_trailing_input() {
        let mut names = NameInterner::new();
        let result = parse_expression_fragment(
            SourceText::new("a;").expect("valid source"),
            SourceId::from_index(4),
            VecTokenSource {
                tokens: vec![
                    token(TokenKind::Identifier, 0, 1),
                    token(TokenKind::Punctuation(Punctuation::Semicolon), 1, 2),
                    token(TokenKind::Eof, 2, 2),
                ],
                index: 0,
            },
            &mut names,
        );

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].kind(),
            ParseDiagnosticKind::UnexpectedToken
        );
    }
}
