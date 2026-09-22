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
            if is_definition_keyword(kind) {
                return true;
            }
            if is_prefix_continuation(kind) {
                continue;
            }
            break;
        }
        false
    }

    /// Parses annotations and modifiers until the definition keyword.
    pub(crate) fn parse_definition_prefix(&mut self) -> DefinitionPrefix {
        let start = self.mark().start();
        let mut prefix = DefinitionPrefix::empty(start);

        loop {
            if is_annotation_start(self) {
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
            self.report(
                ParseDiagnosticKind::UnexpectedToken,
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
        if !matches!(
            self.current().kind,
            TokenKind::Identifier | TokenKind::BackquotedIdentifier
        ) {
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
        } else {
            None
        }
    }

    fn is_deferred_soft_modifier(&mut self) -> bool {
        if !matches!(self.current().kind, TokenKind::Identifier) {
            return false;
        }
        let text = self.current_text().ok();
        matches!(
            text,
            Some("opaque" | "erased" | "tracked" | "into" | "update")
        )
    }

    fn parse_annotation(&mut self) -> TreeId<Untyped> {
        let mark = self.mark();
        self.advance(); // `@` is scanner-facing Operator punctuation.

        let type_mark = self.mark();
        let tpt = self.with_location(Location::Elsewhere, |parser| {
            parser.with_parse_kind(crate::ParseKind::Type, |parser| parser.simple_type())
        });
        let new_tree = self.alloc_from(type_mark, TreeKind::New(New { tpt }));
        let constructor = self.constructor_select(new_tree);
        let mut annotation = self.alloc_from(
            mark,
            TreeKind::Apply(Apply {
                function: constructor,
                args: Vec::new(),
                kind: ApplyKind::Regular,
            }),
        );

        while self.current().kind == TokenKind::Punctuation(Punctuation::LeftParen) {
            annotation = self.parse_application(mark, annotation);
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
        )
    )
}

fn is_prefix_continuation(kind: TokenKind) -> bool {
    is_hard_modifier(kind)
        || matches!(
            kind,
            TokenKind::Identifier
                | TokenKind::BackquotedIdentifier
                | TokenKind::Punctuation(Punctuation::LeftBracket)
        )
}

fn is_soft_modifier<S: dotty_core::TokenSource>(parser: &mut Parser<'_, '_, S>) -> bool {
    parser.soft_modifier().is_some() || parser.is_deferred_soft_modifier()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compilation_unit::tests::{parser_for, token};
    use dotty_core::{NameInterner, TokenKind};

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
}
