use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, Untyped};

use crate::modifiers::DefinitionPrefix;
use crate::{Location, ParseDiagnosticKind, Parser, RecoverySet};

/// A parser-only classification used while building statement sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParsedStatement {
    Definition(TreeId<Untyped>),
    Expression(TreeId<Untyped>),
    Many(Vec<TreeId<Untyped>>),
}

/// The boundary that terminates a statement sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatementSequenceBoundary {
    CompilationUnit,
    Block(TokenKind),
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Parses one statement at the requested source location.
    pub(crate) fn parse_statement(&mut self, location: Location) -> ParsedStatement {
        if self.starts_definition_prefix() {
            let prefix = self.parse_definition_prefix();
            return self.parse_prefixed_definition(location, prefix);
        }
        if matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Val | HardKeyword::Var)
        ) {
            return self.parse_value_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Def) {
            return self.parse_method_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Type) {
            return self.parse_type_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Class) {
            return self.parse_class_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Trait) {
            return self.parse_trait_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Object) {
            return self.parse_object_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Package) {
            return self.parse_package_definition(location);
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Import) {
            return ParsedStatement::Many(self.parse_import_clause(location));
        }
        if self.current().kind == TokenKind::Keyword(HardKeyword::Export) {
            return ParsedStatement::Many(self.parse_export_clause(location));
        }

        let tree = if is_unsupported_start(self.current().kind) {
            self.parse_unsupported_syntax()
        } else {
            self.with_location(location, |parser| parser.expr())
        };
        ParsedStatement::Expression(tree)
    }

    fn parse_prefixed_definition(
        &mut self,
        location: Location,
        prefix: DefinitionPrefix,
    ) -> ParsedStatement {
        match self.current().kind {
            TokenKind::Keyword(HardKeyword::Val | HardKeyword::Var) => {
                self.parse_value_definition_with_prefix(location, prefix)
            }
            TokenKind::Keyword(HardKeyword::Def) => {
                self.parse_method_definition_with_prefix(location, prefix)
            }
            TokenKind::Keyword(HardKeyword::Type) => {
                self.parse_type_definition_with_prefix(location, prefix)
            }
            TokenKind::Keyword(HardKeyword::Class) => {
                self.parse_class_definition_with_prefix(prefix)
            }
            TokenKind::Keyword(HardKeyword::Trait) => {
                self.parse_trait_definition_with_prefix(prefix)
            }
            TokenKind::Keyword(HardKeyword::Object) => {
                self.parse_object_definition_with_prefix(prefix)
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a definition after its annotations and modifiers",
                );
                ParsedStatement::Expression(self.parse_unsupported_syntax())
            }
        }
    }

    /// Parses statements up to a compilation-unit or block boundary.
    pub(crate) fn parse_statement_sequence(
        &mut self,
        boundary: StatementSequenceBoundary,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        let mut statements = Vec::new();
        self.consume_sequence_separators(boundary);

        while !self.sequence_ended(boundary) {
            let checkpoint = self.cursor.checkpoint();
            let location = match boundary {
                StatementSequenceBoundary::CompilationUnit => Location::Elsewhere,
                StatementSequenceBoundary::Block(_) => Location::InBlock,
            };
            statements.push(self.parse_statement(location));

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    match boundary {
                        StatementSequenceBoundary::CompilationUnit => {
                            "parser made no progress while parsing a compilation unit"
                        }
                        StatementSequenceBoundary::Block(_) => {
                            "parser made no progress while parsing a block"
                        }
                    },
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if self.is_sequence_separator(boundary) {
                self.consume_sequence_separators(boundary);
            } else if !self.sequence_ended(boundary) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    match boundary {
                        StatementSequenceBoundary::CompilationUnit => {
                            "expected a statement separator"
                        }
                        StatementSequenceBoundary::Block(_) => {
                            "expected a block statement separator"
                        }
                    },
                );
                self.recover_until(RecoverySet::Statement);
                self.consume_sequence_separators(boundary);
            }
        }

        self.finish_statement_sequence(statements)
    }

    /// Parses a compilation-unit or package statement sequence without the
    /// synthetic trailing expression used by expression blocks.
    pub(crate) fn parse_top_level_sequence(
        &mut self,
        boundary: StatementSequenceBoundary,
    ) -> Vec<TreeId<Untyped>> {
        let mut statements = Vec::new();
        self.consume_sequence_separators(boundary);

        while !self.sequence_ended(boundary) {
            let checkpoint = self.cursor.checkpoint();
            let location = match boundary {
                StatementSequenceBoundary::CompilationUnit => Location::Elsewhere,
                StatementSequenceBoundary::Block(_) => Location::InBlock,
            };
            match self.parse_top_level_statement(location) {
                ParsedStatement::Definition(tree) | ParsedStatement::Expression(tree) => {
                    statements.push(tree)
                }
                ParsedStatement::Many(trees) => statements.extend(trees),
            }

            if !self.cursor.progressed_since(checkpoint) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "parser made no progress while parsing a top-level statement",
                );
                let recovery_checkpoint = self.cursor.checkpoint();
                self.advance();
                if !self.cursor.progressed_since(recovery_checkpoint) {
                    break;
                }
            }

            if self.is_sequence_separator(boundary) {
                self.consume_sequence_separators(boundary);
            } else if !self.sequence_ended(boundary) {
                self.report(
                    ParseDiagnosticKind::UnexpectedToken,
                    "expected a top-level statement separator",
                );
                self.recover_until(RecoverySet::Statement);
                self.consume_sequence_separators(boundary);
            }
        }

        statements
    }

    fn parse_top_level_statement(&mut self, location: Location) -> ParsedStatement {
        if is_top_level_statement_start(self.current().kind) || self.starts_definition_prefix() {
            return self.parse_statement(location);
        }

        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "top-level expressions are not supported in a compilation unit",
        );
        let checkpoint = self.cursor.checkpoint();
        self.with_location(location, |parser| {
            let _ = parser.expr();
        });
        if !self.cursor.progressed_since(checkpoint) && self.current().kind != TokenKind::Eof {
            self.advance();
        }
        ParsedStatement::Expression(self.error_expr(position))
    }

    /// Places definitions in `stats` and leaves only the final expression in `expr`.
    pub(crate) fn finish_statement_sequence(
        &mut self,
        statements: Vec<ParsedStatement>,
    ) -> (Vec<TreeId<Untyped>>, TreeId<Untyped>) {
        let mut stats = Vec::new();
        let mut expression = None;

        for statement in statements {
            if let Some(previous) = expression.take() {
                stats.push(previous);
            }

            match statement {
                ParsedStatement::Definition(tree) => stats.push(tree),
                ParsedStatement::Expression(tree) => expression = Some(tree),
                ParsedStatement::Many(trees) => stats.extend(trees),
            }
        }

        (stats, expression.unwrap_or_else(|| self.synthetic_unit()))
    }

    fn sequence_ended(&self, boundary: StatementSequenceBoundary) -> bool {
        match boundary {
            StatementSequenceBoundary::CompilationUnit => self.current().kind == TokenKind::Eof,
            StatementSequenceBoundary::Block(end) => {
                self.current().kind == end
                    || self.current().kind == TokenKind::Eof
                    || (self.context.case_body && self.is_case_body_terminator())
            }
        }
    }

    fn is_sequence_separator(&self, boundary: StatementSequenceBoundary) -> bool {
        match boundary {
            StatementSequenceBoundary::CompilationUnit => {
                is_statement_separator(self.current().kind)
            }
            StatementSequenceBoundary::Block(_) => is_block_separator(self.current().kind),
        }
    }

    fn consume_sequence_separators(&mut self, boundary: StatementSequenceBoundary) {
        while self.is_sequence_separator(boundary) && !self.sequence_ended(boundary) {
            self.advance();
        }
    }

    fn is_case_body_terminator(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Case)
                | TokenKind::Punctuation(Punctuation::RightBrace)
                | TokenKind::Outdent
        )
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
        self.recover_until(RecoverySet::Statement);
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

pub(crate) const fn is_block_separator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline
            | TokenKind::Newlines
            | TokenKind::Punctuation(Punctuation::Semicolon)
            | TokenKind::Indent
            | TokenKind::Outdent
    )
}

const fn is_unsupported_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            HardKeyword::Match
                | HardKeyword::Val
                | HardKeyword::Var
                | HardKeyword::Enum
                | HardKeyword::Given
        )
    )
}

const fn is_top_level_statement_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            HardKeyword::Val
                | HardKeyword::Var
                | HardKeyword::Def
                | HardKeyword::Type
                | HardKeyword::Class
                | HardKeyword::Trait
                | HardKeyword::Object
                | HardKeyword::Package
                | HardKeyword::Import
                | HardKeyword::Export
                | HardKeyword::Match
                | HardKeyword::Enum
                | HardKeyword::Given
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, TextRange, TokenKind, TreeKind};

    #[test]
    fn statement_sequence_keeps_the_final_expression_as_the_result() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "a\nb",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Newline, 1, 2),
                token(TokenKind::Identifier, 2, 3),
                token(TokenKind::Eof, 3, 3),
            ],
            &mut names,
        );

        let (stats, expr) =
            parser.parse_statement_sequence(StatementSequenceBoundary::CompilationUnit);

        assert_eq!(stats.len(), 1);
        assert!(matches!(parser.ast.get(stats[0]).kind, TreeKind::Ident(_)));
        assert!(matches!(parser.ast.get(expr).kind, TreeKind::Ident(_)));
    }

    #[test]
    fn definition_statements_are_always_block_stats() {
        let mut names = NameInterner::new();
        let x_name = names.intern("x");
        let mut parser = parser_for(
            "x",
            vec![
                token(TokenKind::Identifier, 0, 1),
                token(TokenKind::Eof, 1, 1),
            ],
            &mut names,
        );
        let definition = parser.alloc(
            dotty_core::TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit,
            }),
            Some(parser.current_span()),
        );
        let expression = parser.alloc(
            dotty_core::TreeKind::Ident(dotty_core::ast::Ident {
                name: *dotty_core::TermName::new(x_name).as_name(),
                backquoted: false,
            }),
            Some(parser.current_span()),
        );

        let (stats, expr) = parser.finish_statement_sequence(vec![
            ParsedStatement::Definition(definition),
            ParsedStatement::Expression(expression),
        ]);

        assert_eq!(stats, vec![definition]);
        assert_eq!(expr, expression);
        assert_eq!(
            parser.ast.get(expr).position.unwrap().span().range(),
            TextRange::new(0, 1).unwrap()
        );
    }

    #[test]
    fn preserves_source_order_when_a_definition_follows_an_expression() {
        let mut names = NameInterner::new();
        let first_name = names.intern("first");
        let last_name = names.intern("last");
        let mut parser = parser_for(
            "first\nval x = 1\nlast",
            vec![token(TokenKind::Eof, 21, 21)],
            &mut names,
        );
        let first = parser.alloc(
            dotty_core::TreeKind::Ident(dotty_core::ast::Ident {
                name: *dotty_core::TermName::new(first_name).as_name(),
                backquoted: false,
            }),
            Some(parser.current_span()),
        );
        let definition = parser.alloc(
            dotty_core::TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Unit,
            }),
            Some(parser.current_span()),
        );
        let last = parser.alloc(
            dotty_core::TreeKind::Ident(dotty_core::ast::Ident {
                name: *dotty_core::TermName::new(last_name).as_name(),
                backquoted: false,
            }),
            Some(parser.current_span()),
        );

        let (stats, expr) = parser.finish_statement_sequence(vec![
            ParsedStatement::Expression(first),
            ParsedStatement::Definition(definition),
            ParsedStatement::Expression(last),
        ]);

        assert_eq!(stats, vec![first, definition]);
        assert_eq!(expr, last);
    }
}
