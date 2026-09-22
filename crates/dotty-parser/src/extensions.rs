//! Source-level Scala 3.9 extension definitions.
//!
//! Extension headers deliberately keep their parameter clauses separate from
//! the method trees. The receiver is parsed as an ordinary term parameter,
//! while context clauses before and after it use Dotty's distinct parameter
//! owners.

use dotty_core::ast::{ExtensionMethods, UntypedNode};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::statements::ParsedStatement;
use crate::templates::TemplateBody;
use crate::{Location, ParamOwner, ParseDiagnosticKind, Parser};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_extension_definition(&mut self, _location: Location) -> ParsedStatement {
        let mark = self.mark();
        self.advance(); // contextual `extension`

        let type_params = if self.current().kind == TokenKind::Punctuation(Punctuation::LeftBracket)
        {
            self.parse_type_param_clause(ParamOwner::ExtensionPrefix)
        } else {
            Vec::new()
        };

        let mut param_clauses = Vec::new();
        if !type_params.is_empty() {
            param_clauses.push(type_params);
        }

        while self.current_is_using_parameter_clause() {
            let clause =
                self.parse_single_term_param_clause(ParamOwner::ExtensionPrefix, false, true);
            if !clause.is_empty() {
                param_clauses.push(clause);
            }
        }

        let receiver = if self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen)
            && !self.current_is_using_parameter_clause()
        {
            self.parse_single_term_param_clause(ParamOwner::ExtensionPrefix, true, false)
        } else {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "expected one receiver parameter in an extension",
            );
            Vec::new()
        };
        if receiver.len() != 1 {
            self.report(
                ParseDiagnosticKind::ExpectedToken,
                "an extension must have exactly one receiver parameter",
            );
        } else {
            param_clauses.push(receiver);
        }

        while self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
            let is_using = self.current_is_using_parameter_clause();
            if !is_using {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "only `using` clauses may follow an extension receiver",
                );
            }
            let clause =
                self.parse_single_term_param_clause(ParamOwner::ExtensionFollow, false, is_using);
            if !clause.is_empty() {
                param_clauses.push(clause);
            }
        }

        let methods = self.parse_extension_methods();
        let tree = self.alloc_from(
            mark,
            TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(ExtensionMethods {
                param_clauses,
                methods,
            })),
        );
        ParsedStatement::Definition(tree)
    }

    fn parse_extension_methods(&mut self) -> Vec<TreeId<Untyped>> {
        self.consume_newlines_before_extension_body();

        let methods = match self.current().kind {
            TokenKind::ColonFollow | TokenKind::ColonOp | TokenKind::ColonEol => {
                self.parse_optional_template_body().members
            }
            TokenKind::Punctuation(Punctuation::LeftBrace) => {
                self.parse_template_body(TemplateBody::Braced).members
            }
            TokenKind::Indent => self.parse_template_body(TemplateBody::Indented).members,
            TokenKind::Keyword(HardKeyword::Def) | TokenKind::Keyword(HardKeyword::Export) => {
                self.parse_one_extension_method()
            }
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected extension methods after the extension receiver",
                );
                Vec::new()
            }
        };

        for method in &methods {
            if !matches!(
                self.ast().get(*method).kind,
                TreeKind::DefDef(_) | TreeKind::Export(_)
            ) {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "only methods and exports are allowed inside an extension",
                );
            }
        }
        methods
    }

    fn parse_one_extension_method(&mut self) -> Vec<TreeId<Untyped>> {
        match self.current().kind {
            TokenKind::Keyword(HardKeyword::Export) => self.parse_export_clause(Location::InBlock),
            _ => match self.parse_statement(Location::InBlock) {
                ParsedStatement::Definition(tree) | ParsedStatement::Expression(tree) => {
                    vec![tree]
                }
                ParsedStatement::Many(trees) => trees,
            },
        }
    }

    fn consume_newlines_before_extension_body(&mut self) {
        let mut count = 0;
        while matches!(
            self.cursor.lookahead(count).kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            count += 1;
        }
        if count == 0 {
            return;
        }

        if self.cursor.lookahead(count).kind == TokenKind::Indent {
            for _ in 0..count {
                self.advance();
            }
        } else if self.cursor.lookahead(count).kind
            == TokenKind::Punctuation(Punctuation::LeftBrace)
        {
            for _ in 0..count {
                self.advance();
            }
        } else {
            // Unlike a colon-led template body, an extension body has no
            // token that can eagerly request layout from the scanner. The
            // parser therefore supplies the same feedback before crossing
            // the newline, allowing the scanner to insert `Indent` when the
            // next method is actually more deeply indented.
            self.observe_indented();
            for _ in 0..count {
                self.advance();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{HardKeyword, NameInterner, Punctuation, TokenKind, TreeKind};

    #[test]
    fn parses_an_indented_extension_with_one_receiver_and_method() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "extension (x: X)\n  def foo = x",
            vec![
                token(TokenKind::Identifier, 0, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 12),
                token(TokenKind::ColonFollow, 12, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Punctuation(Punctuation::RightParen), 15, 16),
                token(TokenKind::Indent, 16, 16),
                token(TokenKind::Keyword(HardKeyword::Def), 19, 22),
                token(TokenKind::Identifier, 23, 26),
                token(TokenKind::Operator, 27, 28),
                token(TokenKind::Identifier, 29, 30),
                token(TokenKind::Outdent, 30, 30),
                token(TokenKind::Eof, 30, 30),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(extension) = parser.parse_statement(Location::Elsewhere)
        else {
            panic!("expected an extension definition");
        };

        let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(ref extension)) =
            parser.ast.get(extension).kind
        else {
            panic!("expected ExtensionMethods");
        };
        assert_eq!(extension.param_clauses.len(), 1);
        assert_eq!(extension.param_clauses[0].len(), 1);
        assert_eq!(extension.methods.len(), 1);
        assert!(matches!(
            parser.ast.get(extension.methods[0]).kind,
            TreeKind::DefDef(_)
        ));
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn preserves_extension_using_clauses_before_and_after_the_receiver() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "extension (using context: Ctx) (x: X) (using other: Other) def foo = x",
            vec![
                token(TokenKind::Identifier, 0, 9),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 10, 11),
                token(TokenKind::Identifier, 11, 16),
                token(TokenKind::Identifier, 17, 24),
                token(TokenKind::Punctuation(Punctuation::Colon), 24, 25),
                token(TokenKind::Identifier, 26, 29),
                token(TokenKind::Punctuation(Punctuation::RightParen), 29, 30),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 31, 32),
                token(TokenKind::Identifier, 32, 33),
                token(TokenKind::Punctuation(Punctuation::Colon), 33, 34),
                token(TokenKind::Identifier, 35, 36),
                token(TokenKind::Punctuation(Punctuation::RightParen), 36, 37),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 38, 39),
                token(TokenKind::Identifier, 39, 44),
                token(TokenKind::Identifier, 45, 50),
                token(TokenKind::Punctuation(Punctuation::Colon), 50, 51),
                token(TokenKind::Identifier, 52, 57),
                token(TokenKind::Punctuation(Punctuation::RightParen), 57, 58),
                token(TokenKind::Keyword(HardKeyword::Def), 59, 62),
                token(TokenKind::Identifier, 63, 66),
                token(TokenKind::Operator, 67, 68),
                token(TokenKind::Identifier, 69, 70),
                token(TokenKind::Eof, 70, 70),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(extension) = parser.parse_statement(Location::Elsewhere)
        else {
            panic!("expected an extension definition");
        };
        let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(ref extension)) =
            parser.ast.get(extension).kind
        else {
            panic!("expected ExtensionMethods");
        };

        assert_eq!(extension.param_clauses.len(), 3);
        assert!(extension.param_clauses[0].iter().all(|parameter| {
            matches!(parser.ast.get(*parameter).kind, TreeKind::ValDef(ref value)
                if value.metadata.modifiers.contains(&dotty_core::ast::Modifier::Given))
        }));
        assert!(extension.param_clauses[1].iter().all(|parameter| {
            matches!(parser.ast.get(*parameter).kind, TreeKind::ValDef(ref value)
                if !value.metadata.modifiers.contains(&dotty_core::ast::Modifier::Given))
        }));
        assert!(extension.param_clauses[2].iter().all(|parameter| {
            matches!(parser.ast.get(*parameter).kind, TreeKind::ValDef(ref value)
                if value.metadata.modifiers.contains(&dotty_core::ast::Modifier::Given))
        }));
        assert!(parser.diagnostics().is_empty());
    }
}
