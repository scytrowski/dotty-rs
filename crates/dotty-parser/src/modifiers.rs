//! Parsing of source-level definition prefixes.
//!
//! This module deliberately stops at syntax.  `Modifiers` and annotation
//! trees preserve what was written; semantic flag validation belongs to later
//! compiler phases.

use dotty_core::ast::{Apply, ApplyKind, Modifier, Modifiers, New};
use dotty_core::{HardKeyword, Punctuation, TokenKind, TreeId, TreeKind, Untyped};

use crate::{Location, ParseDiagnosticKind, Parser};

/// Metadata parsed before a source definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DefinitionPrefix {
    pub(crate) start: u32,
    pub(crate) metadata: Modifiers,
}

impl DefinitionPrefix {
    pub(crate) const fn empty(start: u32) -> Self {
        Self {
            start,
            metadata: Modifiers {
                visibility: None,
                modifiers: Vec::new(),
                annotations: Vec::new(),
            },
        }
    }
}

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    /// Returns whether the current statement begins with a definition prefix.
    pub(crate) fn starts_definition_prefix(&mut self) -> bool {
        if is_annotation_start(self) || is_hard_modifier(self.current().kind) {
            return true;
        }

        if !is_soft_modifier(self) {
            return false;
        }

        // A soft keyword is only a modifier if the bounded prefix is followed
        // by a definition keyword.  This keeps `inline`/`open` usable as
        // ordinary term identifiers in expression and value-name positions.
        for offset in 1..=8 {
            let kind = self.cursor.lookahead(offset).kind;
            if is_definition_keyword(kind)
                || (self.context.enum_body && kind == TokenKind::Keyword(HardKeyword::Case))
            {
                return true;
            }
            if is_prefix_continuation(kind) {
                continue;
            }
            break;
        }
        false
    }

    /// Returns whether the current token is a modifier spelling, independent
    /// of whether it starts a definition prefix.
    pub(crate) fn current_is_modifier(&mut self) -> bool {
        is_annotation_start(self)
            || is_hard_modifier(self.current().kind)
            || self.soft_modifier().is_some()
            || self.is_deferred_soft_modifier()
    }

    /// Parses annotations and modifiers until the definition keyword.
    pub(crate) fn parse_definition_prefix(&mut self) -> DefinitionPrefix {
        let start = self.mark().start();
        let mut prefix = DefinitionPrefix::empty(start);

        loop {
            if is_annotation_start(self) {
                if !prefix.metadata.modifiers.is_empty() || prefix.metadata.visibility.is_some() {
                    self.report(
                        ParseDiagnosticKind::UnexpectedToken,
                        "annotations must precede definition modifiers",
                    );
                }
                let annotation = self.parse_annotation();
                prefix.metadata.annotations.push(annotation);
                self.consume_prefix_newlines();
                continue;
            }

            if is_hard_modifier(self.current().kind) {
                let keyword = self.current().kind;
                if matches!(
                    keyword,
                    TokenKind::Keyword(HardKeyword::Private | HardKeyword::Protected)
                ) {
                    self.parse_visibility(&mut prefix.metadata);
                } else if let Some(modifier) = hard_modifier(keyword) {
                    self.add_modifier(&mut prefix.metadata, modifier);
                    self.advance();
                }
                self.consume_prefix_newlines();
                continue;
            }

            if let Some(modifier) = self.soft_modifier() {
                self.add_modifier(&mut prefix.metadata, modifier);
                self.advance();
                self.consume_prefix_newlines();
                continue;
            }

            if self.is_deferred_soft_modifier() {
                self.report(
                    ParseDiagnosticKind::UnsupportedSyntax,
                    "this contextual modifier is not supported yet",
                );
                self.advance();
                self.consume_prefix_newlines();
                continue;
            }

            break;
        }

        prefix
    }

    /// Parses the restricted constructor prefix accepted by Dotty:
    /// annotations followed by an optional access modifier.
    pub(crate) fn parse_constructor_modifiers(&mut self) -> Modifiers {
        let mut metadata = Modifiers::default();
        while is_annotation_start(self) {
            metadata.annotations.push(self.parse_annotation());
        }
        while !metadata.annotations.is_empty()
            && matches!(
                self.current().kind,
                TokenKind::Newline | TokenKind::Newlines
            )
        {
            self.advance();
        }
        if matches!(
            self.current().kind,
            TokenKind::Keyword(HardKeyword::Private | HardKeyword::Protected)
        ) {
            self.parse_visibility(&mut metadata);
        }
        metadata
    }

    fn add_modifier(&mut self, metadata: &mut Modifiers, modifier: Modifier) {
        if metadata.modifiers.contains(&modifier) {
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
                format!("duplicate definition modifier `{modifier:?}`"),
            );
        }
        metadata.modifiers.push(modifier);
    }

    fn parse_visibility(&mut self, metadata: &mut Modifiers) {
        let visibility_span = self.current_span();
        let visibility = match self.current().kind {
            TokenKind::Keyword(HardKeyword::Private) => {
                dotty_core::ast::VisibilitySyntax::Private {
                    qualifier: self.parse_access_qualifier(),
                }
            }
            TokenKind::Keyword(HardKeyword::Protected) => {
                dotty_core::ast::VisibilitySyntax::Protected {
                    qualifier: self.parse_access_qualifier(),
                }
            }
            _ => return,
        };

        if metadata.visibility.is_some() {
            self.report_at(
                ParseDiagnosticKind::UnexpectedToken,
                visibility_span,
                "duplicate definition visibility",
            );
        } else {
            metadata.visibility = Some(visibility);
        }
    }

    fn parse_access_qualifier(&mut self) -> Option<dotty_core::Name> {
        self.advance();
        if !self.accept(TokenKind::Punctuation(Punctuation::LeftBracket)) {
            return None;
        }

        let qualifier = match self.current().kind {
            TokenKind::Identifier
            | TokenKind::BackquotedIdentifier
            | TokenKind::Keyword(HardKeyword::This) => self
                .intern_current_term_name()
                .ok()
                .map(|name| *name.as_name()),
            _ => {
                self.report(
                    ParseDiagnosticKind::ExpectedToken,
                    "expected a visibility qualifier",
                );
                None
            }
        };
        if qualifier.is_some() {
            self.advance();
        }
        self.expect(TokenKind::Punctuation(Punctuation::RightBracket));
        qualifier
    }

    fn consume_prefix_newlines(&mut self) {
        while matches!(
            self.current().kind,
            TokenKind::Newline | TokenKind::Newlines
        ) {
            let checkpoint = self.cursor.checkpoint();
            self.advance();
            if !self.cursor.progressed_since(checkpoint) {
                break;
            }
        }
    }

    fn soft_modifier(&mut self) -> Option<Modifier> {
        if self.current().kind != TokenKind::Identifier {
            return None;
        }
        let name = self.intern_current_term_name().ok()?;
        let known = self.known_names();
        if name == known.inline {
            Some(Modifier::Inline)
        } else if name == known.transparent {
            Some(Modifier::Transparent)
        } else if name == known.open {
            Some(Modifier::Open)
        } else if name == known.infix {
            Some(Modifier::Infix)
        } else if self.starts_opaque_type_definition() {
            Some(Modifier::Opaque)
        } else {
            None
        }
    }

    fn is_deferred_soft_modifier(&mut self) -> bool {
        if !matches!(self.current().kind, TokenKind::Identifier) {
            return false;
        }
        let Ok(name) = self.intern_current_term_name() else {
            return false;
        };
        let known = self.known_names();
        name == known.erased
            || name == known.tracked
            || name == known.into
            || name == known.update
            // `opaque` is a contextual modifier only for `opaque type`, but
            // it must remain deferred elsewhere so unsupported modifier
            // recovery preserves the following definition boundary.
            || name == known.opaque
    }

    fn starts_opaque_type_definition(&mut self) -> bool {
        if self.current().kind != TokenKind::Identifier {
            return false;
        }
        let Ok(name) = self.intern_current_term_name() else {
            return false;
        };
        if name != self.known_names().opaque {
            return false;
        }

        for offset in 1..=8 {
            let kind = self.cursor.lookahead(offset).kind;
            if kind == TokenKind::Keyword(HardKeyword::Type) {
                return true;
            }
            if is_prefix_continuation(kind) {
                continue;
            }
            break;
        }
        false
    }

    pub(crate) fn parse_annotation(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        self.advance(); // `@` is scanner-facing Operator punctuation.

        let tpt = if matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
            self.with_location(Location::Elsewhere, |parser| {
                parser.with_parse_kind(crate::ParseKind::Type, |parser| parser.simple_type())
            })
        } else {
            let position = self.current_span();
            self.report(
                ParseDiagnosticKind::ExpectedType,
                "expected an annotation type after `@`",
            );
            self.error_type(position)
        };
        let new_tree = self.alloc_from(mark, TreeKind::New(New { tpt }));
        let constructor = self.constructor_select(new_tree);
        let mut annotation = constructor;

        while self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
            annotation = self.parse_application(mark, annotation);
        }
        if annotation == constructor {
            annotation = self.alloc_from(
                mark,
                TreeKind::Apply(Apply {
                    function: constructor,
                    args: Vec::new(),
                    kind: ApplyKind::Regular,
                }),
            );
        }
        annotation
    }
}

fn is_annotation_start<S: dotty_core::TokenSource>(parser: &Parser<'_, '_, S>) -> bool {
    parser.current().kind == TokenKind::Operator && parser.current_text_is("@")
}

fn is_hard_modifier(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            HardKeyword::Abstract
                | HardKeyword::Final
                | HardKeyword::Sealed
                | HardKeyword::Implicit
                | HardKeyword::Lazy
                | HardKeyword::Override
                | HardKeyword::Private
                | HardKeyword::Protected
        )
    )
}

fn hard_modifier(kind: TokenKind) -> Option<Modifier> {
    Some(match kind {
        TokenKind::Keyword(HardKeyword::Abstract) => Modifier::Abstract,
        TokenKind::Keyword(HardKeyword::Final) => Modifier::Final,
        TokenKind::Keyword(HardKeyword::Sealed) => Modifier::Sealed,
        TokenKind::Keyword(HardKeyword::Implicit) => Modifier::Implicit,
        TokenKind::Keyword(HardKeyword::Lazy) => Modifier::Lazy,
        TokenKind::Keyword(HardKeyword::Override) => Modifier::Override,
        _ => return None,
    })
}

fn is_definition_keyword(kind: TokenKind) -> bool {
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
                | HardKeyword::Enum
                | HardKeyword::Given
        ) | TokenKind::CaseClass
            | TokenKind::CaseObject
    )
}

fn is_prefix_continuation(kind: TokenKind) -> bool {
    is_hard_modifier(kind)
        || matches!(
            kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::Keyword(HardKeyword::This)
                | TokenKind::Newline
                | TokenKind::Newlines
                | TokenKind::Punctuation(Punctuation::LeftBracket | Punctuation::RightBracket)
        )
}

fn is_soft_modifier<S: dotty_core::TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    parser.soft_modifier().is_some() || parser.is_deferred_soft_modifier()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use crate::statements::ParsedStatement;
    use dotty_core::{NameInterner, TokenKind, TreeKind};

    #[test]
    fn parses_hard_modifiers_in_source_order() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "final lazy def",
            vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::Keyword(HardKeyword::Lazy), 6, 10),
                token(TokenKind::Keyword(HardKeyword::Def), 11, 14),
                token(TokenKind::Eof, 14, 14),
            ],
            &mut names,
        );

        let prefix = parser.parse_definition_prefix();
        assert_eq!(prefix.start, 0);
        assert_eq!(
            prefix.metadata.modifiers,
            vec![Modifier::Final, Modifier::Lazy]
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn parses_qualified_private_visibility() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "private[pkg] val",
            vec![
                token(TokenKind::Keyword(HardKeyword::Private), 0, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 7, 8),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Keyword(HardKeyword::Val), 13, 16),
                token(TokenKind::Eof, 16, 16),
            ],
            &mut names,
        );

        let prefix = parser.parse_definition_prefix();
        let qualifier = match prefix.metadata.visibility {
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier }) => qualifier,
            _ => panic!("expected private visibility"),
        };
        assert!(parser.diagnostics().is_empty());
        drop(parser);
        assert!(matches!(
            qualifier,
            Some(name) if names.resolve(name.text()) == "pkg"
        ));
    }

    #[test]
    fn soft_modifier_is_contextual() {
        let mut names = NameInterner::new();
        let tokens = vec![
            token(TokenKind::Identifier, 0, 6),
            token(TokenKind::Eof, 6, 6),
        ];
        let mut parser = parser_for("inline", tokens, &mut names);
        assert!(!parser.starts_definition_prefix());
    }

    #[test]
    fn opaque_is_a_modifier_only_before_a_type_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "opaque type",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Keyword(HardKeyword::Type), 7, 11),
                token(TokenKind::Eof, 11, 11),
            ],
            &mut names,
        );

        assert!(parser.starts_definition_prefix());
        let prefix = parser.parse_definition_prefix();
        assert_eq!(prefix.metadata.modifiers, vec![Modifier::Opaque]);
        assert_eq!(parser.current().kind, TokenKind::Keyword(HardKeyword::Type));
    }

    #[test]
    fn opaque_remains_an_identifier_outside_a_type_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "opaque value",
            vec![
                token(TokenKind::Identifier, 0, 6),
                token(TokenKind::Identifier, 7, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        assert!(!parser.starts_definition_prefix());
    }

    #[test]
    fn backquoted_soft_modifier_remains_an_identifier() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "`inline` def",
            vec![
                token(TokenKind::BackquotedIdentifier, 0, 8),
                token(TokenKind::Keyword(HardKeyword::Def), 9, 12),
                token(TokenKind::Eof, 12, 12),
            ],
            &mut names,
        );

        assert!(!parser.starts_definition_prefix());
    }

    #[test]
    fn reports_duplicate_modifiers_at_the_repeated_token() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "final final class A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::Keyword(HardKeyword::Final), 6, 11),
                token(TokenKind::Keyword(HardKeyword::Class), 12, 17),
                token(TokenKind::Identifier, 18, 19),
                token(TokenKind::Eof, 19, 19),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(_) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition");
        };
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.diagnostics()[0].span().start(), 6);
    }

    #[test]
    fn malformed_annotation_keeps_the_definition_keyword_available() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "@ class A",
            vec![
                token(TokenKind::Operator, 0, 1),
                token(TokenKind::Keyword(HardKeyword::Class), 2, 7),
                token(TokenKind::Identifier, 8, 9),
                token(TokenKind::Eof, 9, 9),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(_) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected the class to remain parseable");
        };
        assert_eq!(parser.diagnostics().len(), 1);
        assert!(parser.current().kind == TokenKind::Eof);
    }

    #[test]
    fn reports_annotations_after_modifiers() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "final @Ann class A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::Operator, 6, 7),
                token(TokenKind::Identifier, 7, 10),
                token(TokenKind::Keyword(HardKeyword::Class), 11, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Eof, 18, 18),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(_) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition");
        };
        assert_eq!(parser.diagnostics().len(), 1);
        assert_eq!(parser.diagnostics()[0].span().start(), 6);
        assert!(parser.current().kind == TokenKind::Eof);
    }

    #[test]
    fn dispatches_a_prefixed_class_with_source_metadata() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "final class A",
            vec![
                token(TokenKind::Keyword(HardKeyword::Final), 0, 5),
                token(TokenKind::Keyword(HardKeyword::Class), 6, 11),
                token(TokenKind::Identifier, 12, 13),
                token(TokenKind::Eof, 13, 13),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        assert_eq!(definition.metadata.modifiers, vec![Modifier::Final]);
        assert_eq!(
            parser
                .ast()
                .get(id)
                .position
                .unwrap()
                .span()
                .range()
                .start(),
            0
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn builds_an_annotation_as_a_constructor_application() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "@Ann(1) class A",
            vec![
                token(TokenKind::Operator, 0, 1),
                token(TokenKind::Identifier, 1, 4),
                token(TokenKind::Punctuation(Punctuation::LeftParen), 4, 5),
                token(TokenKind::IntegerLiteral, 5, 6),
                token(TokenKind::Punctuation(Punctuation::RightParen), 6, 7),
                token(TokenKind::Keyword(HardKeyword::Class), 8, 13),
                token(TokenKind::Identifier, 14, 15),
                token(TokenKind::Eof, 15, 15),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition");
        };
        let TreeKind::TypeDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a TypeDef");
        };
        let annotation = definition.metadata.annotations[0];
        assert!(matches!(
            parser.ast().get(annotation).kind,
            TreeKind::Apply(_)
        ));
        let TreeKind::Apply(application) = &parser.ast().get(annotation).kind else {
            unreachable!();
        };
        assert_eq!(application.args.len(), 1);
        assert!(matches!(
            parser.ast().get(application.function).kind,
            TreeKind::Select(_)
        ));
        let TreeKind::Select(selection) = &parser.ast().get(application.function).kind else {
            unreachable!();
        };
        assert!(matches!(
            parser.ast().get(selection.qualifier).kind,
            TreeKind::New(_)
        ));
        assert_eq!(
            parser
                .ast()
                .get(annotation)
                .position
                .unwrap()
                .span()
                .range()
                .start(),
            0
        );
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn passes_qualified_visibility_to_a_value_definition() {
        let mut names = NameInterner::new();
        let mut parser = parser_for(
            "private[pkg] val x = 1",
            vec![
                token(TokenKind::Keyword(HardKeyword::Private), 0, 7),
                token(TokenKind::Punctuation(Punctuation::LeftBracket), 7, 8),
                token(TokenKind::Identifier, 8, 11),
                token(TokenKind::Punctuation(Punctuation::RightBracket), 11, 12),
                token(TokenKind::Keyword(HardKeyword::Val), 13, 16),
                token(TokenKind::Identifier, 17, 18),
                token(TokenKind::Operator, 19, 20),
                token(TokenKind::IntegerLiteral, 21, 22),
                token(TokenKind::Eof, 22, 22),
            ],
            &mut names,
        );

        let ParsedStatement::Definition(id) = parser.parse_statement(Location::Elsewhere) else {
            panic!("expected a definition");
        };
        let TreeKind::ValDef(definition) = &parser.ast().get(id).kind else {
            panic!("expected a ValDef");
        };
        assert!(parser.diagnostics().is_empty());
        let qualifier = match definition.metadata.visibility {
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier }) => qualifier,
            _ => panic!("expected private visibility"),
        };
        drop(parser);
        assert!(matches!(
            qualifier,
            Some(name) if names.resolve(name.text()) == "pkg"
        ));
    }
}
