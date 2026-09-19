use dotty_core::ast::{Block, Literal};
use dotty_core::{
    AstArena, Constant, HardKeyword, Punctuation, SourceId, SourceText, TokenKind, TokenSource,
    TreeId, TreeKind, Untyped,
};

use crate::{ParseDiagnostic, ParseDiagnosticKind, Parser};

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
                self.parse_smoke_expr()
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
                | HardKeyword::If
                | HardKeyword::For
                | HardKeyword::While
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
mod tests {
    use super::*;
    use dotty_core::ast::{NumberKind, Parens, This, Tuple, UntypedNode};
    use dotty_core::{
        HardKeyword, NameInterner, SourceId, SourceText, TextRange, Token, TokenSource, TokenValue,
    };

    struct VecTokenSource {
        tokens: Vec<Token>,
        index: usize,
    }

    impl TokenSource for VecTokenSource {
        fn current(&self) -> &Token {
            &self.tokens[self.index]
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

    fn token(kind: TokenKind, start: u32, end: u32) -> Token {
        Token {
            kind,
            span: TextRange::new(start, end).expect("valid test range"),
            value: TokenValue::None,
        }
    }

    fn parser_for<'src, 'names>(
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
    fn parses_an_identifier_with_its_source_span() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Ident(ident) = tree.kind else {
            panic!("expected identifier tree");
        };
        assert_eq!(names.resolve(ident.name.text()), "x");
        assert_eq!(
            tree.position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
        assert!(!ident.backquoted);
    }

    #[test]
    fn parses_a_backquoted_identifier_without_its_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "`x`",
            vec![
                token(TokenKind::BackquotedIdentifier, 0, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Ident(ident) = parser.ast().get(id).kind else {
            panic!("expected identifier tree");
        };

        assert!(ident.backquoted);
        assert_eq!(names.resolve(ident.name.text()), "x");
    }

    #[test]
    fn parses_an_integer_as_a_raw_whole_number() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "42",
            vec![
                token(TokenKind::IntegerLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = tree.kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(10));
        assert_eq!(names.resolve(number.text), "42");
    }

    #[test]
    fn preserves_hexadecimal_integer_radix() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "0xff",
            vec![
                token(TokenKind::IntegerLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = parser.ast().get(id).kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(16));
    }

    #[test]
    fn preserves_binary_integer_radix() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "0b1010",
            vec![
                token(TokenKind::IntegerLiteral, 0, 6),
                token(TokenKind::Eof, 6, 6),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Number(number)) = parser.ast().get(id).kind else {
            panic!("expected raw number tree");
        };
        assert_eq!(number.kind, NumberKind::Whole(2));
    }

    #[test]
    fn decodes_a_decimal_long_literal_as_a_long_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1L",
            vec![
                token(TokenKind::LongLiteral, 0, 2),
                token(TokenKind::Eof, 2, 2),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Long(1)
            })
        ));
    }

    #[test]
    fn decodes_a_hexadecimal_long_literal_as_a_long_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "0xffL",
            vec![
                token(TokenKind::LongLiteral, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Long(255)
            })
        ));
    }

    #[test]
    fn decodes_a_float_suffix_as_a_float_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1.5f",
            vec![
                token(TokenKind::FloatLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Float(value)
            }) if value == 1.5
        ));
    }

    #[test]
    fn decodes_a_double_suffix_as_a_double_constant() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "1.5d",
            vec![
                token(TokenKind::DoubleLiteral, 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Double(value)
            }) if value == 1.5
        ));
    }

    #[test]
    fn parses_a_string_literal_without_retaining_quote_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"hello\"",
            vec![
                token(TokenKind::StringLiteral, 0, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = tree.kind
        else {
            panic!("expected string literal tree");
        };
        assert_eq!(names.resolve(value), "hello");
    }

    #[test]
    fn decodes_escaped_characters_in_a_string_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "\"a\\n\"",
            vec![
                token(TokenKind::StringLiteral, 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "a\n");
    }

    #[test]
    fn decodes_an_uppercase_unicode_escape_in_a_string_literal() {
        let source = r#""\U0041""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "A");
    }

    #[test]
    fn decodes_repeated_uppercase_unicode_escape_prefixes() {
        let source = r#""\UU0041""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "A");
    }

    #[test]
    fn decodes_mixed_case_unicode_escape_prefixes() {
        let source = r#""\uU0041""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "A");
    }

    #[test]
    fn decodes_a_surrogate_pair_to_a_scalar_string_value() {
        let source = r#""\uD834\uDD1E""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected scalar string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "𝄞");
    }

    #[test]
    fn preserves_an_unpaired_surrogate_as_utf16_code_units() {
        let source = r#""a\uD800b""#;
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let tree = parser.ast().get(id).clone();
        drop(parser);

        let TreeKind::Literal(Literal {
            value: Constant::StringUtf16(units),
        }) = tree.kind
        else {
            panic!("expected UTF-16 string literal tree");
        };
        assert_eq!(units, vec!['a' as u16, 0xD800, 'b' as u16]);
    }

    #[test]
    fn preserves_newlines_in_a_triple_quoted_string_literal() {
        let source = "\"\"\"a\nb\"\"\"";
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            source,
            vec![
                token(TokenKind::StringLiteral, 0, source.len() as u32),
                token(TokenKind::Eof, source.len() as u32, source.len() as u32),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Literal(Literal {
            value: Constant::String(value),
        }) = parser.ast().get(id).kind
        else {
            panic!("expected string literal tree");
        };
        drop(parser);

        assert_eq!(names.resolve(value), "a\nb");
    }

    #[test]
    fn parses_true_as_a_boolean_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "true",
            vec![
                token(TokenKind::Keyword(HardKeyword::True), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Boolean(true)
            })
        ));
    }

    #[test]
    fn parses_false_as_a_boolean_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "false",
            vec![
                token(TokenKind::Keyword(HardKeyword::False), 0, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Boolean(false)
            })
        ));
    }

    #[test]
    fn parses_null_as_a_null_literal() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "null",
            vec![
                token(TokenKind::Keyword(HardKeyword::Null), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Null
            })
        ));
    }

    #[test]
    fn parses_this_as_a_this_tree() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "this",
            vec![
                token(TokenKind::Keyword(HardKeyword::This), 0, 4),
                token(TokenKind::Eof, 4, 4),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::This(This { qual: None })
        ));
    }

    #[test]
    fn parses_parenthesized_expression_as_a_parens_node() {
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

        let id = parser.parse_smoke_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Parens(Parens { inner })) =
            parser.ast().get(id).kind
        else {
            panic!("expected parens tree");
        };
        assert!(matches!(parser.ast().get(inner).kind, TreeKind::Ident(_)));
    }

    #[test]
    fn parses_empty_parentheses_as_unit() {
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

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::Literal(Literal {
                value: Constant::Unit
            })
        ));
    }

    #[test]
    fn parses_a_tuple_with_all_element_trees() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "(a, b)",
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

        let id = parser.parse_smoke_expr();

        let TreeKind::PhaseSpecific(UntypedNode::Tuple(Tuple { ref elements })) =
            parser.ast().get(id).kind
        else {
            panic!("expected tuple tree");
        };
        assert_eq!(elements.len(), 2);
        assert!(matches!(
            parser.ast().get(elements[0]).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(elements[1]).kind,
            TreeKind::Ident(_)
        ));
    }

    #[test]
    fn parses_a_simple_selection() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo.bar",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::Identifier, 4, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let (name, qualifier) = match parser.ast().get(id).kind {
            TreeKind::Select(selection) => (selection.name, selection.qualifier),
            _ => panic!("expected selection tree"),
        };
        let qualifier_is_ident = matches!(parser.ast().get(qualifier).kind, TreeKind::Ident(_));
        assert!(parser.diagnostics().is_empty());
        drop(parser);

        assert_eq!(names.resolve(name.text()), "bar");
        assert!(qualifier_is_ident);
    }

    #[test]
    fn parses_a_backquoted_selection_without_its_delimiters() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo.`bar`",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::Dot), 3, 4),
                token(TokenKind::BackquotedIdentifier, 4, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();
        let TreeKind::Select(selection) = parser.ast().get(id).kind else {
            panic!("expected selection tree");
        };

        assert!(selection.backquoted);
        assert_eq!(names.resolve(selection.name.text()), "bar");
    }

    #[test]
    fn parses_a_simple_application_with_an_argument() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo(42)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::IntegerLiteral, 4, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Eof, 7, 7),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert_eq!(application.args.len(), 1);
        assert!(matches!(
            parser.ast().get(application.function).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_empty_application_with_no_arguments() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo()",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::Punctuation(Punctuation::RightParen), 4, 5),
                token(TokenKind::Eof, 5, 5),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert!(application.args.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_an_application_with_multiple_arguments_in_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "foo(1, 2)",
            vec![
                token(TokenKind::Identifier, 0, 3),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 3, 4),
                token(TokenKind::IntegerLiteral, 4, 5),
                token(TokenKind::Punctuation(Punctuation::Comma), 5, 6),
                token(TokenKind::IntegerLiteral, 7, 8),
                token(TokenKind::Punctuation(Punctuation::RightParen), 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        let TreeKind::Apply(application) = &parser.ast().get(id).kind else {
            panic!("expected application tree");
        };
        assert_eq!(application.args.len(), 2);
        assert!(matches!(
            parser.ast().get(application.args[0]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(matches!(
            parser.ast().get(application.args[1]).kind,
            TreeKind::PhaseSpecific(UntypedNode::Number(_))
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn unsupported_expression_input_produces_an_error_tree_and_diagnostic() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "@",
            vec![token(TokenKind::Error, 0, 1), token(TokenKind::Eof, 1, 1)],
            &mut names,
        );

        let id = parser.parse_smoke_expr();

        assert!(matches!(
            parser.ast().get(id).kind,
            TreeKind::PhaseSpecific(UntypedNode::Error(_))
        ));
        assert_eq!(parser.diagnostics().len(), 1);
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
